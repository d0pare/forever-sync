//! Forever Sync's per-machine data folder: the bare repo, backups, and which
//! GitHub repo this machine syncs with. Shared by the CLI and the desktop app.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `~/Library/Application Support/Forever Sync` (macOS) or
/// `%LOCALAPPDATA%\Forever Sync` (Windows).
pub fn default_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("Forever Sync"))
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
