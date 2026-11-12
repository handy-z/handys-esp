#![windows_subsystem = "windows"]
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use handy_sdk::app::args::arg_value;
use handy_sdk::app::self_update::{SelfUpdate, Startup};
use handy_sdk::app::single_instance::SingleInstance;
use handy_sdk::app::status_snapshot::{snapshot_allowed, status_line, wait_for};
use handy_sdk::app::updater_thread::{AppStatus, Updater, production_smoke_test};
use handy_sdk::roblox::http::win_http_get;
use handy_sdk::util::paths::{app_data_dir, set_app_name};
use handy_sdk::util::thread::StopThread;
use handy_sdk::util::timing::TimerResolution;
use handys_esp::app::config::{SettingsStore, TargetStore, load_settings};
use handys_esp::app::dashboard::{DASHBOARD_CLASS, DashboardDeps, run_dashboard};
use handys_esp::app::offset_keys::required_keys;
use handys_esp::app::view::{game_text, marker_line, targets_text};
use handys_esp::esp::engine::Engine;
use handys_esp::esp::targets::Targets;
use handys_esp::game::scanner::{frame_of, scan_roblox};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWDEFAULT;

const MUTEX: &str = "Local\\handys-esp";

fn new_updater() -> Arc<Updater> {
    Arc::new(Updater::new(
        Arc::new(win_http_get),
        production_smoke_test,
        app_data_dir(),
        &required_keys(),
    ))
}

/// `--status <file>`: read-only snapshot of what the ESP sees. Waits for Ready + one frame, or 15 s.
fn status_snapshot(out: &Path) -> i32 {
    if !snapshot_allowed(MUTEX, "handy's esp", out) {
        return 1;
    }
    let ini = app_data_dir().join("config.ini");
    let cfg = load_settings(&ini);
    let store = TargetStore::new(ini);
    let updater = new_updater();
    let u = updater.clone();
    let thread = StopThread::spawn(move |stop| u.run(stop));
    let (mut sess, mut scan, mut frame) = (None, None, None);
    let mut targets = Targets::default();
    let mut took = Duration::ZERO;
    wait_for(
        || {
            if let Some(s) = updater.session()
                && scan.is_none()
                && updater.status() == AppStatus::Ready
            {
                // the first scan finds the game
                targets = store.get(scan_roblox(&s.proc, &s.offsets, &Targets::default()).game);
                let t0 = Instant::now();
                scan = Some(scan_roblox(&s.proc, &s.offsets, &targets));
                took = t0.elapsed();
                sess = Some(s);
            }
            let (Some(sc), Some(s)) = (&scan, &sess) else {
                return false;
            };
            frame = frame_of(&s.proc, &s.offsets, sc, cfg.max_studs);
            frame.is_some()
        },
        Duration::from_secs(15),
        Duration::from_millis(50),
    );
    let mut f = format!(
        "version: {}\nstatus: {}\n",
        env!("CARGO_PKG_VERSION"),
        status_line(&updater)
    );
    if let Some(scan) = &scan {
        let _ = write!(
            f,
            "game: {}\nlooking for: {}\nfound: {}\nplaces to pick from: {}\nscan took: {} ms\n",
            game_text(scan.game),
            targets_text(&targets),
            scan.matches.len(),
            scan.tree.len(),
            took.as_millis()
        );
    }
    if let Some(frame) = &frame {
        let _ = write!(
            f,
            "render size: {} x {}\non screen: {}\n",
            frame.size.x,
            frame.size.y,
            frame.markers.len()
        );
        for m in frame.markers.iter().take(10) {
            let _ = writeln!(f, "  {}", marker_line(m));
        }
    }
    let _ = std::fs::write(out, f);
    thread.request_stop();
    if frame.is_some() { 0 } else { 1 }
}

fn run() -> i32 {
    set_app_name("handy's esp");
    if let Some(out) = arg_value("--status") {
        return if out.is_empty() {
            2
        } else {
            status_snapshot(Path::new(&out))
        };
    }

    // Only builds made by release.cmd update themselves (dev builds and build.cmd never do). Skipped while another
    // copy runs: its exe is in use and it would be replaced under it.
    let enabled = option_env!("HANDY_RELEASE").is_some() && !SingleInstance::is_running(MUTEX);
    let update = match SelfUpdate::start("handy-z", "handys-esp", env!("CARGO_PKG_VERSION"), enabled) {
        Startup::Restarting => return 0, // an update staged earlier is starting; exit before anything else runs
        Startup::Running(u) => u,
    };

    let mut instance = SingleInstance::new(MUTEX, DASHBOARD_CLASS);
    if !instance.acquire() {
        return 0; // brought the running copy's window up
    }
    let _timer = TimerResolution::new(); // 1 ms sleeps for the ~60 Hz engine loop

    let ini = app_data_dir().join("config.ini");
    let store = Arc::new(SettingsStore::new(load_settings(&ini)));
    store.save(&ini); // first run: write the defaults so config.ini exists to edit
    let targets = Arc::new(TargetStore::new(ini.clone()));
    let updater = new_updater();
    let engine = Arc::new(Engine::new(updater.clone(), store.clone(), targets.clone()));
    let updater_thread = {
        let u = updater.clone();
        StopThread::spawn(move |stop| u.run(stop))
    };
    let engine_thread = {
        let e = engine.clone();
        StopThread::spawn(move |stop| e.run(stop))
    };

    let deps = DashboardDeps {
        updater,
        engine,
        settings: store.clone(),
        targets,
        config_ini: ini.clone(),
        update: update.clone(),
    };
    // SW_SHOWDEFAULT: the show command the process was started with
    let rc = run_dashboard(&deps, SW_SHOWDEFAULT);

    drop(engine_thread); // stopped and joined
    store.save(&ini);
    // The Updater join can wait out a WinHTTP timeout (up to 10 s): let a relaunch start meanwhile.
    instance.release();
    drop(updater_thread);
    update.finish(); // after the mutex is released: starts the staged update
    rc
}

fn main() {
    std::process::exit(run());
}
