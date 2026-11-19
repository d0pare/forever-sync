//! Forever Sync's per-machine data folder: the bare repo, backups, and which
//! GitHub repo this machine syncs with. Shared by the CLI and the desktop app.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// App identifier (matches tauri.conf.json); used for the Windows data folder.
pub const APP_ID: &str = "io.github.d0pare.foreversync";

/// `~/Library/Application Support/Forever Sync` (macOS) or
/// `%LOCALAPPDATA%\io.github.d0pare.foreversync` (Windows).
///
/// Windows doesn't use `%LOCALAPPDATA%\Forever Sync` because that is where
/// the per-user installer puts the app itself; keeping data out of the
/// install folder means reinstalling or uninstalling never touches it.
pub fn default_data_dir() -> Option<PathBuf> {
    let base = dirs::data_local_dir()?;
    if cfg!(windows) { Some(base.join(APP_ID)) } else { Some(base.join("Forever Sync")) }
}

/// Moves data written by older Windows builds from `%LOCALAPPDATA%\Forever
/// Sync` (shared with the install folder) into [`default_data_dir`]. Only
/// Forever Sync's own items are moved; the installed app is left alone.
/// No-op elsewhere, or once migrated. Returns what was moved.
pub fn migrate_legacy_data_dir() -> io::Result<Vec<String>> {
    if !cfg!(windows) {
        return Ok(Vec::new());
    }
    let (Some(base), Some(new)) = (dirs::data_local_dir(), default_data_dir()) else { return Ok(Vec::new()) };
    migrate_from(&base.join("Forever Sync"), &new)
}

fn migrate_from(old: &Path, new: &Path) -> io::Result<Vec<String>> {
    if !old.join("repo.git").is_dir() || new.join("repo.git").exists() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(new)?;
    let mut moved = Vec::new();
    for name in ["repo.git", "backups", "config.json", "desktop.json"] {
        let from = old.join(name);
        if from.exists() && !new.join(name).exists() {
            fs::rename(&from, new.join(name))?;
            moved.push(name.to_owned());
        }
    }
    Ok(moved)
}

pub fn repo_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("repo.git")
}

pub fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// The GitHub repo this machine syncs with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoConfig {
    /// `owner/name`
    pub repo: String,
    pub clone_url: String,
}

pub fn load_config(data_dir: &Path) -> io::Result<Option<RepoConfig>> {
    match fs::read_to_string(data_dir.join("config.json")) {
        Ok(s) => serde_json::from_str(&s).map(Some).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn save_config(data_dir: &Path, config: &RepoConfig) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join("config.json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(config).map_err(io::Error::other)?)?;
    fs::rename(tmp, data_dir.join("config.json"))
}

pub fn clear_config(data_dir: &Path) -> io::Result<()> {
    match fs::remove_file(data_dir.join("config.json")) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_only_our_items_once() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("Forever Sync");
        let new = tmp.path().join("io.github.d0pare.foreversync");
        fs::create_dir_all(old.join("repo.git/objects")).unwrap();
        fs::create_dir_all(old.join("backups/apply-1")).unwrap();
        fs::write(old.join("config.json"), "{}").unwrap();
        fs::write(old.join("Forever Sync.exe"), "app").unwrap();
        fs::write(old.join("uninstall.exe"), "un").unwrap();

        let moved = migrate_from(&old, &new).unwrap();
        assert_eq!(moved, ["repo.git", "backups", "config.json"]);
        assert!(new.join("repo.git/objects").is_dir());
        assert!(old.join("Forever Sync.exe").exists() && old.join("uninstall.exe").exists(), "install untouched");
        assert!(migrate_from(&old, &new).unwrap().is_empty(), "second run is a no-op");
    }

    #[test]
    fn config_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_config(tmp.path()).unwrap(), None);
        let c = RepoConfig { repo: "me/forever-sync-data".into(), clone_url: "https://github.com/me/forever-sync-data.git".into() };
        save_config(tmp.path(), &c).unwrap();
        assert_eq!(load_config(tmp.path()).unwrap(), Some(c));
        clear_config(tmp.path()).unwrap();
        clear_config(tmp.path()).unwrap();
        assert_eq!(load_config(tmp.path()).unwrap(), None);
    }
}
