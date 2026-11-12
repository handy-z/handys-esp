use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use handy_sdk::app::updater_thread::{AppStatus, Session, Updater};
use handy_sdk::input::focus::{foreground_is, process_window};
use handy_sdk::input::hooks::KeyPoller;
use handy_sdk::util::thread::StopThread;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

use crate::app::config::{SettingsStore, TargetStore};
use crate::esp::marker::Frame;
use crate::esp::targets::Targets;
use crate::game::scanner::{Scan, frame_of, scan_roblox};
use crate::ui::esp_overlay::{Draw, EspOverlay};

const IDLE_READ_EVERY: Duration = Duration::from_millis(500);

/// A finished scan and the Roblox session it read, kept alive together.
pub struct Scanned {
    pub session: Arc<Session>,
    pub scan: Scan,
}

#[derive(Clone)]
pub struct EngineView {
    pub on: bool,
    /// The current Roblox's last scan; None until one, and while not Ready.
    pub scanned: Option<Arc<Scanned>>,
    /// None while off or the view is unreadable.
    pub frame: Option<Frame>,
    /// Last drawn frame: reading memory, drawing and presenting; 0 = not drawing.
    pub frame_ms: f32,
}

impl Default for EngineView {
    fn default() -> Self {
        Self {
            on: true,
            scanned: None,
            frame: None,
            frame_ms: 0.0,
        }
    }
}

impl EngineView {
    pub fn game(&self) -> i64 {
        self.scanned.as_ref().map_or(0, |s| s.scan.game)
    }
}

#[derive(Default)]
struct Shared {
    scanned: Option<Arc<Scanned>>,
    view: EngineView,
}

/// Two threads, so a slow scan never stalls drawing. The scan thread walks Workspace about once a second (sooner when
/// this game's targets change), also while off so there is something to pick. The engine thread owns the overlay:
/// while on and Roblox is in front it waits for the display to take a frame, then reads the parts and camera, draws
/// and presents at once (one frame a refresh, at most one queued). Only reads Roblox memory; sends no input.
pub struct Engine {
    updater: Arc<Updater>,
    settings: Arc<SettingsStore>,
    targets: Arc<TargetStore>,
    toggle_req: AtomicBool,
    m: Mutex<Shared>,
}

impl Engine {
    pub fn new(updater: Arc<Updater>, settings: Arc<SettingsStore>, targets: Arc<TargetStore>) -> Self {
        Self {
            updater,
            settings,
            targets,
            toggle_req: AtomicBool::new(false),
            m: Mutex::new(Shared::default()),
        }
    }

    pub fn view(&self) -> EngineView {
        self.m.lock().unwrap().view.clone()
    }

    /// Any thread.
    pub fn toggle(&self) {
        self.toggle_req.store(true, Relaxed);
    }

    fn scanned(&self) -> Option<Arc<Scanned>> {
        self.m.lock().unwrap().scanned.clone()
    }

    fn publish(&self, v: EngineView) {
        self.m.lock().unwrap().view = v;
    }

    fn scan_loop(&self, stop: &AtomicBool) {
        let mut last_targets = Targets::default();
        let mut next_scan = Instant::now();
        while !stop.load(Relaxed) {
            let session = self.updater.session();
            match session.filter(|_| self.updater.status() == AppStatus::Ready) {
                None => self.m.lock().unwrap().scanned = None, // also lets a closed Roblox's process handle go
                Some(session) => {
                    let last = self.scanned();
                    let same_session = last.as_ref().is_some_and(|l| Arc::ptr_eq(&l.session, &session));
                    // a new session scans with no targets first: that finds its game, and the next pass uses the
                    // game's targets
                    let targets = match &last {
                        Some(l) if same_session => self.targets.get(l.scan.game),
                        _ => Targets::default(),
                    };
                    let now = Instant::now();
                    if !same_session || targets != last_targets || now >= next_scan {
                        let scan = scan_roblox(&session.proc, &session.offsets, &targets);
                        let next = Arc::new(Scanned { session, scan });
                        last_targets = targets;
                        next_scan = now + Duration::from_secs(1);
                        self.m.lock().unwrap().scanned = Some(next);
                    }
                }
            }
            // a newly ticked name shows within a frame or two of the next pass
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Starts and stops the scan thread itself.
    pub fn run(self: &Arc<Self>, stop: &AtomicBool) {
        let me = self.clone();
        let _scanner = StopThread::spawn(move |s| me.scan_loop(s)); // stopped and joined when run returns
        // drawn from this thread, so a frame goes from memory to the screen with no hand-off
        let mut overlay = EspOverlay::new();
        overlay.create();
        let mut toggle_key = KeyPoller::default(); // a key held at startup is not a press
        let mut on = true;
        let mut roblox: Option<HWND> = None;
        let mut roblox_pid = 0;
        // While not drawing, reads only feed the dashboard counts: twice a second is plenty.
        let mut idle_frame: Option<Frame> = None;
        let mut next_idle_read = Instant::now();
        while !stop.load(Relaxed) {
            overlay.pump();
            let cfg = self.settings.get();
            let session = self.updater.session();
            let pid = session.as_ref().map_or(0, |s| s.proc.pid());
            // only a press made in Roblox toggles, so F2 in another window stays free
            if toggle_key.pressed(cfg.toggle_vk) && foreground_is(pid) {
                on = !on;
            }
            if self.toggle_req.swap(false, Relaxed) {
                on = !on;
            }
            let gone = roblox.is_some_and(|w| !unsafe { IsWindow(Some(w)) }.as_bool());
            if pid != roblox_pid || gone || (roblox.is_none() && pid != 0) {
                roblox_pid = pid;
                roblox = process_window(pid); // the window can appear after the process
            }
            let sc = self.scanned();
            let drawable = match (&sc, roblox) {
                (Some(sc), Some(w)) if on && foreground_is(pid) => Some((sc.clone(), w)),
                _ => None,
            };
            let Some((sc, window)) = drawable else {
                // nothing to draw: still count for the dashboard
                overlay.hide();
                match &sc {
                    Some(sc) if on => {
                        if Instant::now() >= next_idle_read {
                            idle_frame = frame_of(&sc.session.proc, &sc.session.offsets, &sc.scan, cfg.max_studs);
                            next_idle_read = Instant::now() + IDLE_READ_EVERY;
                        }
                    }
                    _ => {
                        idle_frame = None;
                        next_idle_read = Instant::now(); // read at once when it turns on or a scan arrives
                    }
                }
                self.publish(EngineView {
                    on,
                    scanned: sc,
                    frame: idle_frame.clone(),
                    frame_ms: 0.0,
                });
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            // Paced by the display: wait until it can take a frame, then read, draw and present at once, so the frame
            // is as fresh as it can be when it's composed.
            overlay.wait_for_frame();
            let t0 = Instant::now();
            let frame = frame_of(&sc.session.proc, &sc.session.offsets, &sc.scan, cfg.max_studs);
            match &frame {
                Some(f) => overlay.show(
                    window,
                    f,
                    Draw {
                        r#box: cfg.r#box,
                        label: cfg.label,
                        distance: cfg.distance,
                        tracer: cfg.tracer,
                    },
                ),
                None => overlay.hide(),
            }
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            self.publish(EngineView {
                on,
                scanned: Some(sc),
                frame,
                frame_ms: ms,
            });
        }
    }
}
