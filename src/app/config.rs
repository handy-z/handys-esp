use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use handy_sdk::util::ini::Ini;
use handy_sdk::util::shared_settings::{SaveSettings, SharedSettings};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_F2;

use crate::esp::targets::{Targets, join_paths, parse_list, parse_paths, with_path};

pub const MAX_STUDS_LIMIT: i32 = 100000;

/// [esp] in config.ini: how markers look, the same in every game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub toggle_vk: u32,
    pub r#box: bool,
    pub label: bool,
    pub distance: bool,
    pub tracer: bool,
    /// 0 = any distance
    pub max_studs: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            toggle_vk: u32::from(VK_F2.0),
            r#box: true,
            label: true,
            distance: true,
            tracer: true,
            max_studs: 0,
        }
    }
}

/// Every number is clamped to the dashboard's range on load.
pub fn load_settings(file: &Path) -> Settings {
    let ini = Ini::with_section(file, "esp");
    let d = Settings::default();
    Settings {
        toggle_vk: ini.get_int_clamped("toggleVk", d.toggle_vk as i32, 1, 254) as u32,
        r#box: ini.get_bool("box", d.r#box),
        label: ini.get_bool("name", d.label),
        distance: ini.get_bool("distance", d.distance),
        tracer: ini.get_bool("tracer", d.tracer),
        max_studs: ini.get_int_clamped("maxStuds", d.max_studs, 0, MAX_STUDS_LIMIT),
    }
}

pub fn save_settings(file: &Path, s: &Settings) {
    let ini = Ini::with_section(file, "esp");
    ini.put_int("toggleVk", s.toggle_vk as i32);
    ini.put_bool("box", s.r#box);
    ini.put_bool("name", s.label);
    ini.put_bool("distance", s.distance);
    ini.put_bool("tracer", s.tracer);
    ini.put_int("maxStuds", s.max_studs);
}

impl SaveSettings for Settings {
    fn save(&self, ini: &Path) {
        save_settings(ini, self);
    }
}

pub type SettingsStore = SharedSettings<Settings>;

fn game_section(file: &Path, game: i64) -> Ini {
    Ini::with_section(file, &format!("game {game}"))
}

/// What to mark in each Roblox game (DataModel.GameId), in config.ini's [game <id>] sections: paths= (the Pick page),
/// names= and classes= (by hand). Safe from any thread.
pub struct TargetStore {
    cache: Mutex<HashMap<i64, Targets>>,
    ini: PathBuf,
}

impl TargetStore {
    pub fn new(ini: PathBuf) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            ini,
        }
    }

    /// Read from the file once per game, then cached.
    pub fn get(&self, game: i64) -> Targets {
        let mut cache = self.cache.lock().unwrap();
        cache
            .entry(game)
            .or_insert_with(|| {
                let ini = game_section(&self.ini, game);
                Targets {
                    names: parse_list(&ini.get_str("names", "")),
                    classes: parse_list(&ini.get_str("classes", "")),
                    paths: parse_paths(&ini.get_str("paths", "")),
                }
            })
            .clone()
    }

    /// And saves that game's paths=.
    pub fn set_path(&self, game: i64, path: &[String], on: bool) {
        let t = with_path(self.get(game), path, on);
        let mut cache = self.cache.lock().unwrap();
        // shortcut: Ini values stop at 4095 characters (about a hundred picks); move to one key per path if that's hit
        game_section(&self.ini, game).put_str("paths", &join_paths(&t.paths));
        cache.insert(game, t);
    }

    /// Forgets the cache, e.g. after config.ini was edited by hand.
    pub fn reload(&self) {
        self.cache.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F6;

    fn temp_ini(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("handys-esp-rs-{name}-{}.ini", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn strs(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn config_defaults_when_missing() {
        assert_eq!(load_settings(&temp_ini("missing")), Settings::default());
    }

    #[test]
    fn config_round_trips() {
        let p = temp_ini("roundtrip");
        let s = Settings {
            toggle_vk: u32::from(VK_F6.0),
            r#box: false,
            tracer: false,
            max_studs: 500,
            ..Default::default()
        };
        save_settings(&p, &s);
        assert_eq!(load_settings(&p), s);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn config_clamps_hand_edits() {
        let p = temp_ini("clamp");
        std::fs::write(&p, "[esp]\r\nmaxStuds=999999999\r\ntoggleVk=0\r\n").unwrap();
        let s = load_settings(&p);
        assert_eq!(s.max_studs, MAX_STUDS_LIMIT);
        assert_eq!(s.toggle_vk, 1);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn config_targets_per_game() {
        let p = temp_ini("games");
        std::fs::write(
            &p,
            "[game 7]\r\nnames= Chest ,, Coin\r\nclasses=Tool\r\npaths=Map/Coins|Loot\r\n",
        )
        .unwrap();
        let store = TargetStore::new(p.clone());
        let t7 = |paths: &[&[&str]]| Targets {
            names: strs(&["Chest", "Coin"]),
            classes: strs(&["Tool"]),
            paths: paths.iter().map(|p| strs(p)).collect(),
        };
        assert_eq!(store.get(7), t7(&[&["Map", "Coins"], &["Loot"]]));
        assert!(store.get(8).is_empty());
        store.set_path(8, &strs(&["A/B", "C"]), true); // a '/' inside a name survives the file
        store.set_path(7, &strs(&["Loot"]), false);
        store.set_path(7, &strs(&["Map", "Key"]), true);
        let fresh = TargetStore::new(p.clone()); // what was saved; names= and classes= are left as they were
        assert_eq!(fresh.get(7), t7(&[&["Map", "Coins"], &["Map", "Key"]]));
        assert_eq!(
            fresh.get(8),
            Targets {
                paths: vec![strs(&["A/B", "C"])],
                ..Default::default()
            }
        );
        assert!(store.get(9).is_empty()); // now cached
        // a hand edit shows after reload()
        std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"[game 9]\r\npaths=Door\r\n")
            .unwrap();
        assert!(store.get(9).is_empty());
        store.reload();
        assert_eq!(
            store.get(9),
            Targets {
                paths: vec![strs(&["Door"])],
                ..Default::default()
            }
        );
        let _ = std::fs::remove_file(&p);
    }
}
