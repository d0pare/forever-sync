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
///
/// `observed` (the addon list this machine had at its last sync) tells an
/// addon installed here since then apart from one another computer
/// uninstalled.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AddonStatus {
    /// In the synced list but not installed here: install them with WowUp.
    pub missing: Vec<PackageFile>,
    /// Installed here, synced before, and since uninstalled on another
    /// computer (gone from the synced list).
    pub removed_elsewhere: Vec<PackageFile>,
    /// Installed here since the last sync; added to the list at the next one.
    pub new_here: Vec<PackageFile>,
    /// Uninstalled here since the last sync; dropped from the list at the next one.
    pub removed_here: Vec<PackageFile>,
    /// Installed here and in the synced list.
    pub in_sync: usize,
    /// WowUp import string for just the missing addons.
    pub missing_import: Option<String>,
}

pub fn status(synced: &FileSet, observed: Option<&FileSet>, inv: &Inventory) -> AddonStatus {
    let synced = packages_in(synced);
    let local: Vec<PackageFile> = inv.managed.iter().map(PackageFile::from).collect();
    let keys = |v: &[PackageFile]| v.iter().map(PackageFile::key).collect::<BTreeSet<_>>();
    let s_keys = keys(&synced);
    let l_keys = keys(&local);
    let o_keys = keys(&observed.map(packages_in).unwrap_or_default());
    // Listed by WowUp with its folders missing (e.g. mid-update): neither
    // installed nor uninstalled, so it's left out of `missing` and `removed_here`.
    let stale_keys: BTreeSet<_> = inv.stale.iter().map(|a| (a.provider.clone(), a.external_id.clone())).collect();

    let not_installed = |p: &&PackageFile| !l_keys.contains(&p.key());
    let missing: Vec<PackageFile> = synced.iter().filter(not_installed).filter(|p| !o_keys.contains(&p.key())).cloned().collect();
    let removed_here: Vec<PackageFile> = synced
        .iter()
        .filter(not_installed)
        .filter(|p| o_keys.contains(&p.key()) && !stale_keys.contains(&p.key()))
        .cloned()
        .collect();
    let (removed_elsewhere, new_here): (Vec<PackageFile>, Vec<PackageFile>) =
        local.iter().filter(|p| !s_keys.contains(&p.key())).cloned().partition(|p| o_keys.contains(&p.key()));
    let in_sync = local.len() - removed_elsewhere.len() - new_here.len();
    let missing_import = (!missing.is_empty()).then(|| import_for(&missing));
    AddonStatus { missing, removed_elsewhere, new_here, removed_here, in_sync, missing_import }
}

/// Where one addon stands, for the addon list UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonState {
    InSync,
    Missing,
    RemovedElsewhere,
    NewHere,
    RemovedHere,
    /// WowUp lists it but its folders are missing right now (updating).
    Updating,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddonRow {
    pub name: String,
    pub provider: String,
    pub id: String,
    pub state: AddonState,
    /// Installed version here (WowUp), if installed.
    pub version_here: Option<String>,
    /// Version in the synced list (the last computer that changed it).
    pub version_synced: Option<String>,
    /// The synced list has a different release than this computer.
    pub other_version: bool,
    pub folders: Vec<String>,
}

/// Every addon in the synced list or installed here, with its state,
/// sorted by name.
pub fn overview(synced: &FileSet, observed: Option<&FileSet>, inv: &Inventory) -> Vec<AddonRow> {
    let st = status(synced, observed, inv);
    let key = |p: &PackageFile| p.key();
    let in_list = |v: &[PackageFile]| v.iter().map(key).collect::<BTreeSet<_>>();
    let (missing, elsewhere, new_here, removed_here) =
        (in_list(&st.missing), in_list(&st.removed_elsewhere), in_list(&st.new_here), in_list(&st.removed_here));
    let synced: std::collections::BTreeMap<_, _> = packages_in(synced).into_iter().map(|p| (p.key(), p)).collect();
    let local: std::collections::BTreeMap<_, _> =
        inv.managed.iter().map(PackageFile::from).map(|p| (p.key(), p)).collect();

    let keys: BTreeSet<_> = synced.keys().chain(local.keys()).cloned().collect();
    let mut rows: Vec<AddonRow> = keys
        .into_iter()
        .map(|k| {
            let (s, l) = (synced.get(&k), local.get(&k));
            let state = if missing.contains(&k) {
                AddonState::Missing
            } else if elsewhere.contains(&k) {
                AddonState::RemovedElsewhere
            } else if new_here.contains(&k) {
                AddonState::NewHere
            } else if removed_here.contains(&k) {
                AddonState::RemovedHere
            } else if l.is_none() {
                AddonState::Updating
            } else {
                AddonState::InSync
            };
            let any = l.or(s).expect("key from one of the maps");
            AddonRow {
                name: any.name.clone(),
                provider: k.0.clone(),
                id: k.1.clone(),
                state,
                version_here: l.and_then(|p| p.version.clone()),
                version_synced: s.and_then(|p| p.version.clone()),
                other_version: matches!((s, l), (Some(s), Some(l)) if s.version_id != l.version_id),
                folders: any.folders.clone(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    rows
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
            stale: vec![],
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

        // The Mac synced Questie and OldThing before; OldThing has since been
        // dropped from the list, and NewHere was installed after the last sync.
        let observed = files_from_inventory(&inv(vec![
            addon("Questie", "WowUpHub", "2", "11"),
            addon("OldThing", "Curse", "9", "1"),
        ]));
        let mac = inv(vec![
            addon("Questie", "WowUpHub", "2", "11"),
            addon("OldThing", "Curse", "9", "2"),
            addon("NewHere", "Curse", "7", "1"),
        ]);
        let s = status(&synced, Some(&observed), &mac);
        let names = |v: &[PackageFile]| v.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&s.missing), ["Plater"]);
        assert_eq!(names(&s.removed_elsewhere), ["OldThing"], "a version change doesn't matter");
        assert_eq!(names(&s.new_here), ["NewHere"], "new here is not removed elsewhere");
        assert_eq!(s.in_sync, 1, "version differences still count as installed");
        assert!(s.missing_import.is_some());
        assert!(status(&synced, None, &pc).missing_import.is_none());

        // Without an observed list nothing counts as removed elsewhere.
        let first = status(&synced, None, &mac);
        assert!(first.removed_elsewhere.is_empty());
        assert_eq!(names(&first.new_here), ["OldThing", "NewHere"]);
    }

    #[test]
    fn overview_labels_every_addon() {
        let synced = files_from_inventory(&inv(vec![
            addon("Plater", "Curse", "1", "v2"),
            addon("Questie", "Curse", "2", "1"),
            addon("Updating", "Curse", "5", "1"),
        ]));
        let observed = files_from_inventory(&inv(vec![
            addon("Plater", "Curse", "1", "v1"),
            addon("Gone", "Curse", "3", "1"),
            addon("Updating", "Curse", "5", "1"),
        ]));
        let mut here = inv(vec![addon("Plater", "Curse", "1", "v1"), addon("Gone", "Curse", "3", "1"), addon("Fresh", "Curse", "4", "1")]);
        here.stale = vec![addon("Updating", "Curse", "5", "1")];
        let rows: Vec<_> = overview(&synced, Some(&observed), &here).into_iter().map(|r| (r.name, r.state, r.other_version)).collect();
        assert_eq!(
            rows,
            [
                ("Fresh".to_owned(), AddonState::NewHere, false),
                ("Gone".to_owned(), AddonState::RemovedElsewhere, false),
                ("Plater".to_owned(), AddonState::InSync, true),
                ("Questie".to_owned(), AddonState::Missing, false),
                ("Updating".to_owned(), AddonState::Updating, false),
            ]
        );
    }

    #[test]
    fn uninstalled_here_or_updating_is_not_missing() {
        let synced = files_from_inventory(&inv(vec![addon("Plater", "Curse", "1", "v1"), addon("Questie", "Curse", "2", "1")]));
        let observed = synced.clone();
        let mut here = inv(vec![]);
        here.stale = vec![addon("Questie", "Curse", "2", "1")]; // WowUp is replacing its folders
        let s = status(&synced, Some(&observed), &here);
        assert!(s.missing.is_empty());
        assert_eq!(s.removed_here.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Plater"]);
    }
}
