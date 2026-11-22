//! Removing addons that were uninstalled on another machine (WowUp's import
//! adds addons but never removes them).
//!
//! Safety rules:
//! * only WowUp-managed addons that are gone from the synced list;
//! * never a folder still used by another addon that stays (e.g. a library
//!   folder shared between packages), never an unmanaged folder;
//! * folders are **moved** to `_forever_/Interface/.foreversync-trash/<time>/`
//!   (same drive, so instant; WoW only loads `Interface/AddOns`), not deleted;
//!   trash older than [`TRASH_KEEP`] is purged;
//! * callers must check the game isn't running (Windows may hold files open).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use foreversync_addons::Inventory;
use serde::Serialize;

use crate::FileSet;
use crate::addon_sync::{self, PackageFile};

pub const TRASH_DIR: &str = ".foreversync-trash";
pub const TRASH_KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct RemovalPlan {
    /// Addons to remove (installed here, gone from the synced list).
    pub packages: Vec<PackageFile>,
    /// AddOns folders that will be moved to trash.
    pub folders: Vec<String>,
    /// Folders of those addons kept because another addon still uses them.
    pub kept_shared: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RemovalReport {
    pub moved: Vec<String>,
    pub trash_dir: Option<PathBuf>,
}

/// What removing the uninstalled-elsewhere addons would do. `observed` is
/// this machine's addon list at its last sync (see [`addon_sync::status`]).
/// `only` limits it to those `(provider, id)` keys; `None` means all of them.
pub fn plan(
    synced: &FileSet,
    observed: Option<&FileSet>,
    local: &Inventory,
    only: Option<&[(String, String)]>,
) -> RemovalPlan {
    let status = addon_sync::status(synced, observed, local);
    let packages: Vec<PackageFile> = status
        .removed_elsewhere
        .into_iter()
        .filter(|p| only.is_none_or(|keys| keys.iter().any(|(pr, id)| *pr == p.provider && *id == p.id)))
        .collect();
    let removing: BTreeSet<(&str, &str)> = packages.iter().map(|p| (p.provider.as_str(), p.id.as_str())).collect();

    // Folders still needed by any addon that stays (installed or synced).
    let mut needed: BTreeSet<String> = local
        .managed
        .iter()
        .filter(|a| !removing.contains(&(a.provider.as_str(), a.external_id.as_str())))
        .flat_map(|a| a.folders.iter().cloned())
        .collect();
    needed.extend(addon_sync::packages_in(synced).into_iter().flat_map(|p| p.folders));

    let mut folders = BTreeSet::new();
    let mut kept_shared = BTreeSet::new();
    for f in packages.iter().flat_map(|p| p.folders.iter()) {
        if needed.contains(f) { kept_shared.insert(f.clone()) } else { folders.insert(f.clone()) };
    }
    RemovalPlan { packages, folders: folders.into_iter().collect(), kept_shared: kept_shared.into_iter().collect() }
}

/// A plain folder name directly inside AddOns (no separators, not hidden).
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
}

/// Moves the planned folders out of `addons_dir` into a new trash folder.
pub fn execute(addons_dir: &Path, plan: &RemovalPlan) -> io::Result<RemovalReport> {
    let mut report = RemovalReport::default();
    if plan.folders.is_empty() {
        return Ok(report);
    }
    let interface = addons_dir.parent().ok_or_else(|| io::Error::other("AddOns folder has no parent"))?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let trash = interface.join(TRASH_DIR).join(stamp.to_string());
    for name in &plan.folders {
        if !safe_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("refusing odd folder name {name:?}")));
        }
        let from = addons_dir.join(name);
        match fs::symlink_metadata(&from) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue, // already gone
            Err(e) => return Err(e),
        }
        fs::create_dir_all(&trash)?;
        fs::rename(&from, trash.join(name))?;
        report.moved.push(name.clone());
        report.trash_dir = Some(trash.clone());
    }
    Ok(report)
}

/// Deletes trash batches older than `keep`. Returns how many were removed.
pub fn purge_trash(addons_dir: &Path, keep: Duration) -> io::Result<usize> {
    let Some(interface) = addons_dir.parent() else { return Ok(0) };
    let root = interface.join(TRASH_DIR);
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let mut removed = 0;
    for entry in entries.filter_map(Result::ok) {
        let Some(stamp) = entry.file_name().to_str().and_then(|s| s.parse::<u128>().ok()) else { continue };
        if now_ms.saturating_sub(stamp) > keep.as_millis() {
            fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use foreversync_addons::ManagedAddon;

    fn addon(name: &str, id: &str, folders: &[&str]) -> ManagedAddon {
        ManagedAddon {
            name: name.into(),
            provider: "Curse".into(),
            external_id: id.into(),
            version_id: None,
            version: Some("1".into()),
            folders: folders.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn inv(addons: Vec<ManagedAddon>) -> Inventory {
        Inventory {
            wowup_dir: PathBuf::from("/w"),
            installation_id: "i".into(),
            managed: addons,
            unmanaged_folders: vec!["HandMade".into()],
            stale: vec![],
            dropped_records: 0,
        }
    }

    #[test]
    fn plans_only_uninstalled_and_keeps_shared_folders() {
        // Synced list (the source machine): Plater + Questie.
        let synced = addon_sync::files_from_inventory(&inv(vec![
            addon("Plater", "1", &["Plater"]),
            addon("Questie", "2", &["Questie", "QuestieDB", "LibShared"]),
        ]));
        // This machine also has Extra (which shares LibShared) and Gone.
        let local = inv(vec![
            addon("Plater", "1", &["Plater"]),
            addon("Questie", "2", &["Questie", "QuestieDB", "LibShared"]),
            addon("Extra", "3", &["Extra", "Extra_Options", "LibShared"]),
            addon("Gone", "4", &["Gone"]),
        ]);
        // Both were synced from here before, so they were removed elsewhere.
        let observed = addon_sync::files_from_inventory(&local);
        let p = plan(&synced, Some(&observed), &local, None);
        assert_eq!(p.packages.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Extra", "Gone"]);
        assert_eq!(p.folders, ["Extra", "Extra_Options", "Gone"]);
        assert_eq!(p.kept_shared, ["LibShared"]);

        let only = plan(&synced, Some(&observed), &local, Some(&[("Curse".into(), "4".into())]));
        assert_eq!(only.folders, ["Gone"]);

        // Never synced from here: new installs, not removals.
        assert!(plan(&synced, None, &local, None).folders.is_empty());
    }

    #[test]
    fn moves_to_trash_and_purges_old_batches() {
        let tmp = tempfile::tempdir().unwrap();
        let addons = tmp.path().join("Interface/AddOns");
        for f in ["Extra", "Extra_Options", "Keep", "HandMade"] {
            fs::create_dir_all(addons.join(f)).unwrap();
            fs::write(addons.join(f).join(format!("{f}.toc")), "## Title: x").unwrap();
        }
        let p = RemovalPlan {
            packages: vec![],
            folders: vec!["Extra".into(), "Extra_Options".into(), "AlreadyGone".into()],
            kept_shared: vec![],
        };
        let r = execute(&addons, &p).unwrap();
        assert_eq!(r.moved, ["Extra", "Extra_Options"]);
        let trash = r.trash_dir.unwrap();
        assert!(trash.join("Extra/Extra.toc").exists());
        assert!(!addons.join("Extra").exists());
        assert!(addons.join("Keep").exists() && addons.join("HandMade").exists());

        // Nothing is old enough yet; with zero retention the batch goes.
        assert_eq!(purge_trash(&addons, TRASH_KEEP).unwrap(), 0);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(purge_trash(&addons, Duration::from_millis(1)).unwrap(), 1);
        assert!(!trash.exists());
    }

    #[test]
    fn refuses_unsafe_names() {
        let tmp = tempfile::tempdir().unwrap();
        let addons = tmp.path().join("Interface/AddOns");
        fs::create_dir_all(&addons).unwrap();
        for bad in ["..", "../WTF", ".hidden", "a/b", "C:x"] {
            let p = RemovalPlan { packages: vec![], folders: vec![bad.into()], kept_shared: vec![] };
            assert!(execute(&addons, &p).is_err(), "{bad}");
        }
    }
}
