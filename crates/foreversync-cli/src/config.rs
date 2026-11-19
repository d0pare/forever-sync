//! Which GitHub repo this machine syncs with (`<data-dir>/config.json`).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// `owner/name`
    pub repo: String,
    pub clone_url: String,
}

pub fn load(data_dir: &Path) -> Result<Option<Config>> {
    match fs::read_to_string(data_dir.join("config.json")) {
        Ok(s) => Ok(Some(serde_json::from_str(&s).context("config.json")?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn save(data_dir: &Path, config: &Config) -> Result<()> {
    fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join("config.json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(config)?)?;
    fs::rename(tmp, data_dir.join("config.json"))?;
    Ok(())
}
