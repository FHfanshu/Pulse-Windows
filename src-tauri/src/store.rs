//! Persistence under `%APPDATA%\Pulse` (upstream: UserDefaults).

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::placement::Placement;

fn dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

pub fn load_placement(app: &AppHandle) -> Placement {
    dir(app)
        .and_then(|d| std::fs::read(d.join("placement.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_placement(app: &AppHandle, placement: &Placement) {
    if let Some(d) = dir(app) {
        if let Ok(bytes) = serde_json::to_vec_pretty(placement) {
            let _ = std::fs::write(d.join("placement.json"), bytes);
        }
    }
}
