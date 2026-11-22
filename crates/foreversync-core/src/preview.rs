//! What a machine's first sync would do, worked out before anything changes
//! (DESIGN.md §7.5: the first apply on a new machine always shows a preview).
//!
//! It runs the same steps as [`Engine::sync`] (adopt the cloud history,
//! build the snapshot for each [`FirstSync`] choice, plan the apply) without
//! committing or writing. Only the remote-tracking ref moves (the fetch).

use foreversync_addons::load_inventory;
use foreversync_git::Remote;
use serde::Serialize;

use crate::history::{self, SettingsFile, Status};
use crate::sync::{Engine, FirstSync, Result};
use crate::{Changes, FileSet, addon_sync, apply, layout};

#[derive(Debug, Serialize)]
pub struct FirstSyncPreview {
    /// The cloud already has Forever Sync data (this is an additional
    /// machine), not just an empty repo or one with only a README.
    pub cloud_has_data: bool,
    /// This install has synced before, so the first-sync choice no longer
    /// applies; both effects then describe a normal sync.
    pub synced_before: bool,
    pub use_cloud: Effect,
    pub upload_local: Effect,
    /// Settings files that are already identical here and in the cloud.
    pub same: usize,
    /// Addons in the cloud's list that this computer's WowUp doesn't have
    /// (`None` if WowUp can't be read).
    pub addons_missing: Option<usize>,
}

/// What one choice changes. File statuses are relative to each side:
/// `added` is new there, `modified` replaces that side's version.
#[derive(Debug, Default, Serialize)]
pub struct Effect {
    /// Files written to (or removed from) this computer's `WTF` folder.
    pub this_computer: Vec<SettingsFile>,
    /// Files sent to the cloud, and from there to your other computers.
    pub cloud: Vec<SettingsFile>,
}

impl Engine<'_> {
    /// Fetches and works out what each first-sync choice would do.
    pub fn preview_first_sync(&self, remote: Option<&Remote>) -> Result<FirstSyncPreview> {
        let remote_head = match remote {
            Some(r) => self.store.fetch(r)?,
            None => None,
        };
        // `sync` adopts the cloud history when this machine has none.
        let head = match self.store.head()? {
            Some(h) => Some(h),
            None => remote_head,
        };
        let head_files = match head {
            Some(h) => self.store.files_at(&h.to_string())?,
            None => FileSet::new(),
        };
        let observed = self.store.files_at_ref(&self.disk_ref())?;
        let scanned = self.observe(observed.as_ref())?;

        let effect = |first: FirstSync| -> Result<Effect> {
            let (next, _) = self.next_state(&head_files, observed.as_ref(), &scanned.files, first);
            // After the snapshot, the observed disk state is `scanned`.
            let wtf_dir = self.install.wtf_dir();
            let plan = apply::plan(&wtf_dir, &next, Some(&scanned.files))?;
            let mut this_computer: Vec<SettingsFile> = plan
                .writes
                .iter()
                .filter_map(|p| {
                    let exists = apply::disk_path(&wtf_dir, p).is_some_and(|d| d.exists());
                    history::describe(p, if exists { Status::Modified } else { Status::Added })
                })
                .collect();
            this_computer.extend(plan.deletes.iter().filter_map(|p| history::describe(p, Status::Deleted)));
            let up = Changes::between(&layout::wtf_only(&head_files), &layout::wtf_only(&next));
            Ok(Effect { this_computer, cloud: describe_all(&up) })
        };

        let local = layout::wtf_only(&scanned.files);
        let same = local.iter().filter(|(p, b)| head_files.get(*p) == Some(*b)).count();
        let addons_missing = if self.wowup_dirs.is_empty() {
            None
        } else {
            load_inventory(&self.wowup_dirs, &self.install.flavor_dir, &self.install.addons_dir())
                .ok()
                .map(|inv| addon_sync::status(&head_files, observed.as_ref(), &inv).missing.len())
        };
        Ok(FirstSyncPreview {
            cloud_has_data: remote_head.is_some() && head_files.contains_key("foreversync.json"),
            synced_before: observed.is_some(),
            use_cloud: effect(FirstSync::UseCloud)?,
            upload_local: effect(FirstSync::UploadLocal)?,
            same,
            addons_missing,
        })
    }
}

fn describe_all(c: &Changes) -> Vec<SettingsFile> {
    let tagged = [(&c.added, Status::Added), (&c.modified, Status::Modified), (&c.deleted, Status::Deleted)];
    tagged
        .into_iter()
        .flat_map(|(paths, st)| paths.iter().filter_map(move |p| history::describe(p, st)))
        .collect()
}
