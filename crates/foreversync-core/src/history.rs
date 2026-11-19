//! Readable history of the data repo for the app's History screen:
//! who/what/why per snapshot, changed settings grouped by addon and
//! character, addon-list changes, and conflicts recorded in merges.

use foreversync_git::{PathStatus, Store, StoreError};
use serde::Serialize;

use crate::FileSet;
use crate::addon_sync::{PACKAGES_PREFIX, PackageFile};
use crate::layout::WTF_PREFIX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Snapshot,
    Merge,
    Restore,
    AddonsSource,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConflictNote {
    pub path: String,
    /// "local" or "remote", from the machine that made the merge.
    pub kept: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub id: String,
    pub time: i64,
    pub summary: String,
    pub kind: Kind,
    pub machine: Option<String>,
    pub trigger: Option<String>,
    pub settings_changed: usize,
    pub addons_changed: usize,
    pub conflicts: Vec<ConflictNote>,
    pub parents: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Added,
    Modified,
    Deleted,
}

impl From<PathStatus> for Status {
    fn from(s: PathStatus) -> Self {
        match s {
            PathStatus::Added => Status::Added,
            PathStatus::Modified => Status::Modified,
            PathStatus::Deleted => Status::Deleted,
        }
    }
}

/// Where a settings file lives, for display and grouping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsFile {
    pub path: String,
    pub status: Status,
    pub file: String,
    pub account: Option<String>,
    pub realm: Option<String>,
    pub character: Option<String>,
    /// Addon whose SavedVariables this is (`Plater` for `.../SavedVariables/Plater.lua`).
    pub addon: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddonChange {
    pub name: String,
    pub status: Status,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryDetails {
    pub entry: HistoryEntry,
    pub settings: Vec<SettingsFile>,
    pub addons: Vec<AddonChange>,
}

fn trailer<'a>(message: &'a str, key: &str) -> Option<&'a str> {
    message.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')).map(str::trim))
}

fn entry(store: &Store, id: &str) -> Result<HistoryEntry, StoreError> {
    let c = store.commit(id)?;
    let changes = store.changes(&c.id)?;
    let trigger = trailer(&c.message, "ForeverSync-Trigger").map(str::to_owned);
    let kind = if c.parents.len() > 1 {
        Kind::Merge
    } else if trailer(&c.message, "ForeverSync-Restore").is_some() {
        Kind::Restore
    } else if trigger.as_deref() == Some("addons-source") {
        Kind::AddonsSource
    } else if c.summary.starts_with("Snapshot") {
        Kind::Snapshot
    } else {
        Kind::Other
    };
    let conflicts = c
        .message
        .lines()
        .filter_map(|l| l.strip_prefix("ForeverSync-Conflict:"))
        .filter_map(|r| {
            let r = r.trim();
            let (path, kept) = r.rsplit_once(" (kept ")?;
            Some(ConflictNote { path: path.to_owned(), kept: kept.trim_end_matches(')').to_owned() })
        })
        .collect();
    // Machine: trailer, else the quoted host in the summary.
    let machine = trailer(&c.message, "ForeverSync-Machine")
        .map(str::to_owned)
        .or_else(|| c.summary.split('"').nth(1).map(str::to_owned));
    Ok(HistoryEntry {
        settings_changed: changes.iter().filter(|p| p.path.starts_with(WTF_PREFIX)).count(),
        addons_changed: changes.iter().filter(|p| p.path.starts_with(PACKAGES_PREFIX)).count(),
        id: c.id,
        time: c.time,
        summary: c.summary,
        kind,
        machine,
        trigger,
        conflicts,
        parents: c.parents,
    })
}

/// The newest `limit` commits on `main`, newest first.
pub fn list(store: &Store, limit: usize) -> Result<Vec<HistoryEntry>, StoreError> {
    store.log(limit)?.iter().map(|c| entry(store, &c.id)).collect()
}

/// Describes a repo path under `wtf/` for display.
pub fn describe(path: &str, status: Status) -> Option<SettingsFile> {
    let rel = path.strip_prefix(WTF_PREFIX)?;
    let parts: Vec<&str> = rel.split('/').collect();
    let file = (*parts.last()?).to_owned();
    let addon_of = |dir: &str| {
        (dir == "SavedVariables")
            .then(|| file.strip_suffix(".lua").or_else(|| file.strip_suffix(".LUA")).map(str::to_owned))
            .flatten()
    };
    let (account, realm, character, addon) = match parts.as_slice() {
        ["Account", "SavedVariables", _] => (None, None, None, addon_of("SavedVariables")),
        ["Account", acct, _] => (Some(*acct), None, None, None),
        ["Account", acct, d, _] => (Some(*acct), None, None, addon_of(d)),
        ["Account", acct, realm, ch, _] => (Some(*acct), Some(*realm), Some(*ch), None),
        ["Account", acct, realm, ch, d, _] => (Some(*acct), Some(*realm), Some(*ch), addon_of(d)),
        _ => (None, None, None, None),
    };
    Some(SettingsFile {
        path: path.to_owned(),
        status,
        file,
        account: account.map(str::to_owned),
        realm: realm.map(str::to_owned),
        character: character.map(str::to_owned),
        addon,
    })
}

/// What one commit changed, readably.
pub fn details(store: &Store, id: &str) -> Result<HistoryDetails, StoreError> {
    let entry = entry(store, id)?;
    let parent = entry.parents.first().cloned();
    let mut settings = Vec::new();
    let mut addons = Vec::new();
    for ch in store.changes(&entry.id)? {
        let status = Status::from(ch.status);
        if let Some(s) = describe(&ch.path, status) {
            settings.push(s);
        } else if ch.path.starts_with(PACKAGES_PREFIX) {
            // Deleted entries are read from the parent to get the name.
            let rev = if status == Status::Deleted { parent.clone() } else { Some(entry.id.clone()) };
            let pkg: Option<PackageFile> = rev
                .and_then(|r| store.file_at(&r, &ch.path).ok().flatten())
                .and_then(|b| serde_json::from_slice(&b).ok());
            if let Some(p) = pkg {
                addons.push(AddonChange { name: p.name, status, version: p.version });
            }
        }
    }
    settings.sort_by(|a, b| a.addon.cmp(&b.addon).then(a.path.cmp(&b.path)));
    addons.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(HistoryDetails { entry, settings, addons })
}

/// For a conflict recorded in merge `entry`, the parent commit holding the
/// version that was *not* kept (merge parents are [local, remote]).
pub fn other_side<'a>(entry: &'a HistoryEntry, conflict: &ConflictNote) -> Option<&'a str> {
    let idx = if conflict.kept == "local" { 1 } else { 0 };
    entry.parents.get(idx).map(String::as_str)
}

/// Every SavedVariables file of `addon` (all accounts and characters) in `files`.
pub fn addon_settings_paths(files: &FileSet, addon: &str) -> Vec<String> {
    files
        .keys()
        .filter(|p| describe(p, Status::Modified).and_then(|d| d.addon).is_some_and(|a| a.eq_ignore_ascii_case(addon)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_paths() {
        let d = describe("wtf/Account/ACCT/93/Char-One/SavedVariables/PlaterDBChr.lua", Status::Added).unwrap();
        assert_eq!(
            (d.account.as_deref(), d.realm.as_deref(), d.character.as_deref(), d.addon.as_deref()),
            (Some("ACCT"), Some("93"), Some("Char-One"), Some("PlaterDBChr"))
        );
        let d = describe("wtf/Account/ACCT/SavedVariables/Plater.lua", Status::Modified).unwrap();
        assert_eq!((d.character, d.addon.as_deref()), (None, Some("Plater")));
        let d = describe("wtf/Account/ACCT/bindings-cache.wtf", Status::Modified).unwrap();
        assert_eq!((d.account.as_deref(), d.addon), (Some("ACCT"), None));
        let d = describe("wtf/Account/SavedVariables/Blizzard_SavedSets.lua", Status::Modified).unwrap();
        assert_eq!((d.account, d.addon.as_deref()), (None, Some("Blizzard_SavedSets")));
        assert!(describe("addons/packages/curse-1.json", Status::Added).is_none());
    }

    #[test]
    fn finds_addon_settings_across_characters() {
        let files: FileSet = [
            "wtf/Account/A/SavedVariables/Plater.lua",
            "wtf/Account/A/93/C1/SavedVariables/Plater.lua",
            "wtf/Account/A/93/C1/SavedVariables/PlaterDBChr.lua",
            "wtf/Account/A/93/C1/AddOns.txt",
        ]
        .iter()
        .map(|p| (p.to_string(), Vec::new()))
        .collect();
        assert_eq!(
            addon_settings_paths(&files, "plater"),
            ["wtf/Account/A/93/C1/SavedVariables/Plater.lua", "wtf/Account/A/SavedVariables/Plater.lua"]
        );
    }
}
