//! The WowUp addon **list** in the data repo (DESIGN.md §6).
//!
//! Each WowUp-managed addon is one small JSON file under `addons/packages/`.
//! Keeping one file per addon means the ordinary per-file merge already does
//! the right thing: additions from both machines are combined, an uninstall
//! is a deletion that propagates, and an addon this machine simply hasn't
//! installed yet is never treated as removed (local changes are measured
//! against what WowUp reported last time, see `sync.rs`).
//!
//! Addon files themselves are never stored or touched; WowUp installs them.

use std::collections::BTreeSet;

use foreversync_addons::{ImportPayload, Inventory, ManagedAddon, import_string};
use serde::{Deserialize, Serialize};

use crate::FileSet;

pub const PACKAGES_PREFIX: &str = "addons/packages/";

/// One addon as stored in the repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageFile {
    pub name: String,
    /// WowUp provider, e.g. `Curse`, `WowUpHub`.
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub folders: Vec<String>,
}

impl PackageFile {
    fn key(&self) -> (String, String) {
        (self.provider.clone(), self.id.clone())
    }
}

impl From<&ManagedAddon> for PackageFile {
    fn from(a: &ManagedAddon) -> Self {
        Self {
            name: a.name.clone(),
            provider: a.provider.clone(),
            id: a.external_id.clone(),
            version: a.version.clone(),
            version_id: a.version_id.clone(),
            folders: a.folders.clone(),
        }
    }
}

impl From<&PackageFile> for ManagedAddon {
    fn from(p: &PackageFile) -> Self {
        Self {
            name: p.name.clone(),
            provider: p.provider.clone(),
            external_id: p.id.clone(),
            version_id: p.version_id.clone(),
            version: p.version.clone(),
            folders: p.folders.clone(),
        }
    }
}

/// Repo path for an addon: `addons/packages/<provider>-<id>.json`.
pub fn package_path(provider: &str, id: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '_' })
            .collect()
    };
    format!("{PACKAGES_PREFIX}{}-{}.json", clean(provider), clean(id))
}

/// The repo files describing a WowUp inventory.
pub fn files_from_inventory(inv: &Inventory) -> FileSet {
    inv.managed
        .iter()
        .map(|a| {
            let pkg = PackageFile::from(a);
            let mut json = serde_json::to_string_pretty(&pkg).expect("package serializes");
            json.push('\n');
            (package_path(&pkg.provider, &pkg.id), json.into_bytes())
        })
        .collect()
}

/// Addons listed in a repo file set, sorted by name.
pub fn packages_in(files: &FileSet) -> Vec<PackageFile> {
    let mut out: Vec<PackageFile> = files
        .iter()
        .filter(|(p, _)| p.starts_with(PACKAGES_PREFIX) && p.ends_with(".json"))
        .filter_map(|(_, b)| serde_json::from_slice(b).ok())
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Only the addon-list part of a file set.
pub fn addon_files(files: &FileSet) -> FileSet {
    files.iter().filter(|(p, _)| p.starts_with(PACKAGES_PREFIX)).map(|(p, b)| (p.clone(), b.clone())).collect()
}

/// How this machine's WowUp compares with the synced list.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AddonStatus {
    /// In the synced list but not installed here: install them with WowUp.
    pub missing: Vec<PackageFile>,
    /// Installed here but removed from the synced list (uninstalled elsewhere).
    pub removed_elsewhere: Vec<PackageFile>,
    /// Installed here and in the synced list.
    pub in_sync: usize,
    /// WowUp import string for just the missing addons.
    pub missing_import: Option<String>,
}

pub fn status(synced: &FileSet, local: &Inventory) -> AddonStatus {
    let synced = packages_in(synced);
    let local: Vec<PackageFile> = local.managed.iter().map(PackageFile::from).collect();
    let synced_keys: BTreeSet<_> = synced.iter().map(PackageFile::key).collect();
    let local_keys: BTreeSet<_> = local.iter().map(PackageFile::key).collect();

    let missing: Vec<PackageFile> = synced.iter().filter(|p| !local_keys.contains(&p.key())).cloned().collect();
    let removed_elsewhere: Vec<PackageFile> = local.iter().filter(|p| !synced_keys.contains(&p.key())).cloned().collect();
    let in_sync = local.len() - removed_elsewhere.len();
    let missing_import = (!missing.is_empty()).then(|| import_for(&missing));
    AddonStatus { missing, removed_elsewhere, in_sync, missing_import }
}

/// WowUp import string for `packages`.
pub fn import_for(packages: &[PackageFile]) -> String {
    let addons: Vec<ManagedAddon> = packages.iter().map(ManagedAddon::from).collect();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    import_string(&ImportPayload::from_addons(&addons, now_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn addon(name: &str, provider: &str, id: &str, ver: &str) -> ManagedAddon {
        ManagedAddon {
            name: name.into(),
            provider: provider.into(),
            external_id: id.into(),
            version_id: Some(format!("{id}-{ver}")),
            version: Some(ver.into()),
            folders: vec![name.replace(' ', "")],
        }
    }

    fn inv(addons: Vec<ManagedAddon>) -> Inventory {
        Inventory {
            wowup_dir: PathBuf::from("/w"),
            installation_id: "i".into(),
            managed: addons,
            unmanaged_folders: vec![],
            dropped_records: 0,
        }
    }

    #[test]
    fn paths_are_safe_and_stable() {
        assert_eq!(package_path("Curse", "1076354"), "addons/packages/curse-1076354.json");
        assert_eq!(package_path("Git Hub", "a/b..c"), "addons/packages/git_hub-a_b__c.json");
    }

    #[test]
    fn round_trip_and_status() {
        let pc = inv(vec![addon("Plater", "Curse", "1", "v1"), addon("Questie", "WowUpHub", "2", "12")]);
        let synced = files_from_inventory(&pc);
        assert_eq!(packages_in(&synced).len(), 2);

        let mac = inv(vec![addon("Questie", "WowUpHub", "2", "11"), addon("OldThing", "Curse", "9", "1")]);
        let s = status(&synced, &mac);
        assert_eq!(s.missing.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Plater"]);
        assert_eq!(s.removed_elsewhere.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["OldThing"]);
        assert_eq!(s.in_sync, 1, "version differences still count as installed");
        assert!(s.missing_import.is_some());
        assert!(status(&synced, &pc).missing_import.is_none());
    }
}
