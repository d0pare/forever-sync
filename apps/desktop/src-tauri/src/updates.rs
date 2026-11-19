//! Auto-update from GitHub Releases (tauri-plugin-updater). Checks on start
//! and every 6 hours; installing is always the user's click, never during a
//! sync. Releases are built and signed by .github/workflows/release.yml.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::state::AppState;

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub version: String,
    pub notes: Option<String>,
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio_sleep(FIRST_CHECK_AFTER).await;
        loop {
            let _ = check(&app).await;
            tokio_sleep(CHECK_EVERY).await;
        }
    });
}

async fn tokio_sleep(d: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

/// Checks GitHub for a newer release; remembers and announces it.
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let info = update.map(|u| UpdateInfo { current: u.current_version.clone(), version: u.version.clone(), notes: u.body.clone() });
    *app.state::<AppState>().update.lock().unwrap() = info.clone();
    let _ = app.emit("update", &info);
    Ok(info)
}

/// Downloads, verifies and installs the latest release, then restarts.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Already up to date.")?;
    // Download (and verify the signature) first; then install while holding
    // the sync lock so the app isn't swapped out from under a running sync.
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    {
        let state = app.state::<AppState>();
        let _guard = state.sync_lock.lock().unwrap();
        update.install(bytes).map_err(|e| e.to_string())?;
    }
    app.restart();
}
