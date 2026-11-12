//! Everything but the entry point, so it can be unit-tested.

pub mod app {
    pub mod config;
    pub mod dashboard;
    pub mod offset_keys;
    pub mod view;
}
pub mod esp {
    pub mod engine;
    pub mod marker;
    pub mod targets;
}
pub mod game {
    pub mod scanner;
}
pub mod ui {
    pub mod esp_overlay;
}
