//! Per-install sync state: the commit WTF/ last matched (DESIGN.md §7.2 step 7).
//! Used as the `base` that decides which files apply may delete.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use foreversync_core::ForeverInstall;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    installs: BTreeMap<String, InstallState>,
}

#[derive(Default, Serialize, Deserialize)]
struct InstallState {
    base: Option<String>,
}

fn load(data_dir: &Path) -> Result<State> {
    match fs::read_to_string(data_dir.join("state.json")) {
        Ok(s) => Ok(serde_json::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}

fn key(install: &ForeverInstall) -> String {
    install.flavor_dir.display().to_string()
}

pub fn base(data_dir: &Path, install: &ForeverInstall) -> Result<Option<String>> {
    Ok(load(data_dir)?.installs.get(&key(install)).and_then(|s| s.base.clone()))
}

pub fn set_base(data_dir: &Path, install: &ForeverInstall, commit: &str) -> Result<()> {
    let mut state = load(data_dir)?;
    state.installs.entry(key(install)).or_default().base = Some(commit.to_owned());
    fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join("state.json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&state)?)?;
    fs::rename(tmp, data_dir.join("state.json"))?;
    Ok(())
}
