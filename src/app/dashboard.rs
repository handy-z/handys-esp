use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use handy_sdk::app::self_update::SelfUpdate;
use handy_sdk::app::updater_thread::{Updater, status_name, status_tone};
use handy_sdk::input::key::vk_name;
use handy_sdk::ui::dashboard::{Dashboard, Spec, TreeItem, check, note, number, page, section, text, tree, value};
use handy_sdk::ui::overlay::StatusOverlay;
use windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD;

use crate::app::config::{MAX_STUDS_LIMIT, Settings, SettingsStore, TargetStore, load_settings};
use crate::app::view::{PickNode, found_text, game_text, pick_tree, targets_text};
use crate::esp::engine::{Engine, Scanned};
use crate::esp::targets::Targets;

pub const DASHBOARD_CLASS: &str = "handys-esp-dashboard";

const ID_ON: i32 = 2;
const ID_TARGETS: i32 = 3;
const ID_FOUND: i32 = 4;
const ID_BOX: i32 = 5;
const ID_LABEL: i32 = 6;
const ID_DISTANCE: i32 = 7;
const ID_TRACER: i32 = 8;
const ID_MAX: i32 = 9;
const ID_HINT: i32 = 10;
const ID_GAME: i32 = 11;
const ID_FIND: i32 = 12;
const ID_PICK: i32 = 13;

pub struct DashboardDeps {
    pub updater: Arc<Updater>,
    pub engine: Arc<Engine>,
    pub settings: Arc<SettingsStore>,
    pub targets: Arc<TargetStore>,
    pub config_ini: PathBuf,
    pub update: Arc<SelfUpdate>,
}

fn write_time(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn same(a: &Option<Arc<Scanned>>, b: &Option<Arc<Scanned>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (a, b) => a.is_none() && b.is_none(),
    }
}

/// Shows the dashboard and draws the ESP overlay over Roblox until the window is closed. UI thread only.
pub fn run_dashboard(deps: &DashboardDeps, show: SHOW_WINDOW_CMD) -> i32 {
    // declared before the dashboard, whose callbacks borrow them
    let picks: RefCell<Vec<PickNode>> = RefCell::new(Vec::new()); // what the tree shows, so a click maps back to a path
    let picked_from: RefCell<Option<Arc<Scanned>>> = RefCell::new(None);
    let picked_for = RefCell::new(Targets::default());
    let (find, picked_find) = (RefCell::new(String::new()), RefCell::new(String::new()));
    let pick_game = Cell::new(0i64);
    let pill = StatusOverlay::new();
    let ini_time = Cell::new(write_time(&deps.config_ini));
    let next_ini_check = Cell::new(Instant::now());
    let first = Cell::new(true);

    let d = Dashboard::new(Spec {
        title: "handy's esp".into(),
        badge: deps.update.badge().text,
        window_class: DASHBOARD_CLASS.into(),
        rows: vec![
            page("ESP"),
            section("ESP"),
            check(ID_ON, "ESP"),
            value(ID_TARGETS, "Looking for").lines(2),
            value(ID_FOUND, "Found"),
            section("Draw"),
            check(ID_BOX, "Box"),
            check(ID_LABEL, "Name"),
            check(ID_DISTANCE, "Distance"),
            check(ID_TRACER, "Tracer line"),
            number(ID_MAX, "Max studs (0 = any)", 0, MAX_STUDS_LIMIT),
            note(ID_HINT),
            page("Pick"),
            section("Tick a place to mark it (a folder: what's in it)"),
            value(ID_GAME, "Game"),
            text(ID_FIND, "Find"),
            tree(ID_PICK, 18),
        ],
        hint_row: Some(ID_HINT),
        ..Default::default()
    }); // the ESP overlay is drawn by the engine thread, not from this tick
    pill.create();

    d.on_tick(|ui| {
        let b = deps.update.badge();
        ui.set_badge(&b.text, b.tone, &b.tip);
        let now = Instant::now();
        if now >= next_ini_check.get() {
            // config.ini may be edited by hand (classes= has no UI)
            next_ini_check.set(now + Duration::from_secs(1));
            let t = write_time(&deps.config_ini);
            if t != ini_time.get() {
                ini_time.set(t);
                let loaded = load_settings(&deps.config_ini);
                deps.settings.update(|s| *s = loaded);
                deps.targets.reload();
                ui.set_number(ID_MAX, loaded.max_studs);
            }
        }
        let st: Settings = deps.settings.get();
        if first.replace(false) {
            ui.set_number(ID_MAX, st.max_studs);
        }
        let v = deps.engine.view();
        let status = deps.updater.status();
        ui.set_status(status_tone(status), status_name(status), &deps.updater.detail());
        ui.set_text(ID_ON, &format!("ESP\tpress {}", vk_name(st.toggle_vk)));
        ui.set_check(ID_ON, v.on);
        ui.set_check(ID_BOX, st.r#box);
        ui.set_check(ID_LABEL, st.label);
        ui.set_check(ID_DISTANCE, st.distance);
        ui.set_check(ID_TRACER, st.tracer);
        let targets = deps.targets.get(v.game());
        ui.set_text(ID_TARGETS, &targets_text(&targets));
        ui.set_text(ID_FOUND, &found_text(&v));
        ui.set_text(ID_GAME, &game_text(v.game()));
        pill.set(if v.on { "ESP on" } else { "ESP off" }, v.on);
        // a new scan about once a second, a tick, or a key in Find
        if !same(&v.scanned, &picked_from.borrow())
            || targets != *picked_for.borrow()
            || *find.borrow() != *picked_find.borrow()
        {
            *picked_from.borrow_mut() = v.scanned.clone();
            *picked_for.borrow_mut() = targets.clone();
            *picked_find.borrow_mut() = find.borrow().clone();
            pick_game.set(v.game());
            *picks.borrow_mut() = match &v.scanned {
                Some(sc) => pick_tree(&sc.scan.tree, &targets, &find.borrow()),
                None => Vec::new(),
            };
            let items: Vec<TreeItem> = picks.borrow().iter().map(|p| p.item.clone()).collect();
            ui.set_tree(ID_PICK, &items);
        }
    });
    d.on_badge_click(|ui| {
        if deps.update.request_restart() {
            ui.close();
        } else {
            handy_sdk::ui::open_url(&deps.update.release_url());
        }
    });
    d.on_check(|_, id, checked| {
        if id == ID_ON {
            return deps.engine.toggle();
        }
        deps.settings.update(|s| {
            *(match id {
                ID_BOX => &mut s.r#box,
                ID_LABEL => &mut s.label,
                ID_DISTANCE => &mut s.distance,
                _ => &mut s.tracer,
            }) = checked;
        });
        deps.settings.save(&deps.config_ini);
    });
    d.on_tree_check(|_, _, index, checked| {
        let picks = picks.borrow();
        if let Some(p) = picks.get(index)
            && pick_game.get() != 0
            && !p.path.is_empty()
        {
            deps.targets.set_path(pick_game.get(), &p.path, checked);
        }
    });
    d.on_text(|ui, _, s| {
        s.clone_into(&mut find.borrow_mut());
        ui.tick(); // filter now, not on the next tick
    });
    d.on_number(|_, id, value| {
        if id != ID_MAX {
            return;
        }
        deps.settings.update(|s| s.max_studs = value);
        deps.settings.save(&deps.config_ini);
    });
    d.run(show)
}
