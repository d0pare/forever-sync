//! Core of Forever Sync: finding the WoW Forever install, deciding what to
//! sync, scanning `WTF/` and writing repo state back with backups.

pub mod apply;
pub mod discovery;
pub mod forever;
pub mod layout;
pub mod lua;
pub mod process;
pub mod rules;
pub mod scan;

use std::collections::BTreeMap;

pub use discovery::{Account, Character, DiscoveryError, ForeverInstall, Realm};

/// Repo-relative path (`/`-separated, NFC) → file bytes. Same shape as
/// `foreversync_git::FileSet`.
pub type FileSet = BTreeMap<String, Vec<u8>>;

/// Paths added, modified and deleted going from `old` to `new`.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Changes {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

impl Changes {
    pub fn between(old: &FileSet, new: &FileSet) -> Self {
        let mut c = Changes::default();
        for (path, bytes) in new {
            match old.get(path) {
                None => c.added.push(path.clone()),
                Some(prev) if prev != bytes => c.modified.push(path.clone()),
                Some(_) => {}
            }
        }
        c.deleted = old.keys().filter(|p| !new.contains_key(*p)).cloned().collect();
        c
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}
