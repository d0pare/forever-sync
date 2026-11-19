//! Shared app state and the persisted desktop settings.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use foreversync_github::DeviceCode;
use serde::{Deserialize, Serialize};

/// What the UI and tray show about syncing. Sent to the UI as `status` events.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub syncing: bool,
    pub game_running: bool,
    pub last_sync: Option<LastSync>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LastSync {
    /// Unix seconds.
    pub at: i64,
    pub trigger: String,
    /// Local files recorded in this sync's snapshot.
    pub local_changes: usize,
    /// Took changes from another machine (fast-forward or merge).
    pub took_remote: bool,
    pub conflicts: Vec<String>,
    pub pushed: bool,
    /// Files written / deleted in WTF/, if anything was applied.
    pub written: usize,
    pub deleted: usize,
    /// Incoming changes wait because the game is running.
    pub pending_apply: bool,
    pub skipped: Vec<String>,
    /// Addons in the synced list that this machine's WowUp doesn't have.
    pub addons_missing: usize,
    /// Addons installed here that were uninstalled on another machine.
    pub addons_removed_elsewhere: usize,
    /// Addon-list entries uploaded from this machine.
    pub addon_changes: usize,
}

/// Desktop-only settings, kept in `<data dir>/desktop.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DesktopSettings {
    /// Set after the first sync from onboarding; background syncing waits for it.
    #[serde(default)]
    pub onboarded: bool,
}

impl DesktopSettings {
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(data_dir.join("desktop.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let tmp = data_dir.join("desktop.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, data_dir.join("desktop.json"))
    }
}

pub struct AppState {
    pub data_dir: PathBuf,
    pub status: Mutex<Status>,
    /// Serializes sync cycles (UI button, tray, background worker).
    pub sync_lock: Mutex<()>,
    pub pending_login: Mutex<Option<DeviceCode>>,
    /// GitHub login of the signed-in user, once known.
    pub user: Mutex<Option<String>>,
    /// Repo whose privacy was last confirmed, and when (re-checked hourly).
    pub privacy_checked: Mutex<Option<(String, Instant)>>,
    pub tray_status: Mutex<Option<tauri::menu::MenuItem<tauri::Wry>>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            status: Mutex::new(Status::default()),
            sync_lock: Mutex::new(()),
            pending_login: Mutex::new(None),
            user: Mutex::new(None),
            privacy_checked: Mutex::new(None),
            tray_status: Mutex::new(None),
        }
    }
}
