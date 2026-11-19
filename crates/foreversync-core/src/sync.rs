//! The sync cycle (DESIGN.md §7.2): fetch → snapshot → merge → push → apply.
//!
//! Local changes are detected against the **last observed disk state**
//! (kept under a local-only ref per install), not against `main`. So:
//! * files in `main` that this machine hasn't applied yet (game running) are
//!   never reverted or "deleted" by a snapshot;
//! * a character folder that exists only on another machine is never treated
//!   as deleted here.

use std::path::{Path, PathBuf};

use foreversync_addons::{default_wowup_dirs, load_inventory};
use foreversync_git::{Oid, PushOutcome, Remote, Store, StoreError};
use serde::Serialize;

use crate::merge::{Conflict, Side, merge3};
use crate::{Changes, FileSet, ForeverInstall, addon_sync, apply, layout, process, scan};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("the remote kept moving; gave up after {0} attempts")]
    TooManyRetries(u32),
    #[error("can't read WowUp's addon list, so it can't be made the source: {0}")]
    AddonsUnavailable(String),
}

pub type Result<T> = std::result::Result<T, SyncError>;

/// How to resolve a file both machines changed differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum ConflictPolicy {
    /// The side whose latest snapshot is newer wins (default).
    #[default]
    NewestWins,
    PreferLocal,
    PreferRemote,
}

/// What a machine's first snapshot does with files the cloud already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum FirstSync {
    /// Cloud versions win; local-only files are added (recommended).
    #[default]
    UseCloud,
    /// This machine's files win.
    UploadLocal,
}

#[derive(Debug, Default, Serialize)]
pub struct SnapshotOutcome {
    pub commit: Option<String>,
    /// WTF/ changes recorded.
    pub changes: Changes,
    /// Addon-list changes recorded (`addons/packages/...`).
    pub addon_changes: Changes,
    /// Why the WowUp list couldn't be read (the last known list was kept).
    pub addons_error: Option<String>,
    pub skipped: Vec<scan::Skipped>,
    /// Local edits that replaced a not-yet-applied remote change.
    pub overrode_pending: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub enum Integration {
    NoRemote,
    UpToDate,
    /// Took the remote history as-is.
    FastForward,
    /// Local is ahead; nothing to take.
    LocalAhead,
    Merged { conflicts: Vec<Conflict> },
}

#[derive(Debug, Serialize)]
pub enum ApplyOutcome {
    UpToDate,
    /// Not written because the Forever client is running; retried later.
    GameRunning,
    Applied(apply::ApplyReport),
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub snapshot: SnapshotOutcome,
    pub integration: Integration,
    pub pushed: bool,
    pub apply: ApplyOutcome,
    /// With `addons_source`: how the synced addon list changed to match
    /// this machine (e.g. addons that came only from other machines removed).
    pub addon_list_reset: Option<Changes>,
}

pub struct Engine<'a> {
    pub install: &'a ForeverInstall,
    pub store: &'a Store,
    pub backup_root: PathBuf,
    pub host: String,
    /// WowUp data folders to read the addon list from (empty: don't).
    pub wowup_dirs: Vec<PathBuf>,
    /// Make this machine's WowUp list the synced addon list (drop entries
    /// that came only from other machines). Off by default.
    pub addons_source: bool,
}

/// What this machine has right now: syncable WTF/ files plus the WowUp
/// addon list, as repo paths.
struct Observed {
    files: FileSet,
    skipped: Vec<scan::Skipped>,
    addons_error: Option<String>,
}

impl<'a> Engine<'a> {
    pub fn new(install: &'a ForeverInstall, store: &'a Store, backup_root: &Path) -> Self {
        Self {
            install,
            store,
            backup_root: backup_root.to_path_buf(),
            host: process::host_name(),
            wowup_dirs: default_wowup_dirs(),
            addons_source: false,
        }
    }

    /// Replaces the addon list on `main` with exactly this machine's WowUp
    /// list. Refuses (rather than emptying the list) if WowUp can't be read.
    fn reset_addon_list(&self) -> Result<Changes> {
        let inv = load_inventory(&self.wowup_dirs, &self.install.flavor_dir, &self.install.addons_dir())
            .map_err(|e| SyncError::AddonsUnavailable(e.to_string()))?;
        let head = self.store.head_files()?;
        let mine = addon_sync::files_from_inventory(&inv);
        let changes = Changes::between(&addon_sync::addon_files(&head), &mine);
        if !changes.is_empty() {
            let mut next: FileSet =
                head.into_iter().filter(|(p, _)| !p.starts_with(addon_sync::PACKAGES_PREFIX)).collect();
            next.extend(mine);
            let msg = format!(
                "Make \"{}\"'s addon list the source\n\nForeverSync-Machine: {}\nForeverSync-Trigger: addons-source\n",
                self.host, self.host
            );
            self.store.snapshot(&next, &msg)?;
        }
        Ok(changes)
    }

    /// Scans WTF/ and reads WowUp. If WowUp can't be read, the addon part of
    /// `previous` is kept so nothing looks uninstalled.
    fn observe(&self, previous: Option<&FileSet>) -> Result<Observed> {
        let scanned = scan::scan_wtf(&self.install.wtf_dir())?;
        let mut files = scanned.files;
        let addons_error = if self.wowup_dirs.is_empty() {
            files.extend(previous.map(addon_sync::addon_files).unwrap_or_default());
            None
        } else {
            match load_inventory(&self.wowup_dirs, &self.install.flavor_dir, &self.install.addons_dir()) {
                Ok(inv) => {
                    files.extend(addon_sync::files_from_inventory(&inv));
                    None
                }
                Err(e) => {
                    files.extend(previous.map(addon_sync::addon_files).unwrap_or_default());
                    Some(e.to_string())
                }
            }
        };
        Ok(Observed { files, skipped: scanned.skipped, addons_error })
    }

    /// Local-only ref holding the last observed state of this install's WTF/.
    pub fn disk_ref(&self) -> String {
        // FNV-1a: stable across Rust versions, unlike DefaultHasher.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in self.install.flavor_dir.to_string_lossy().as_bytes() {
            h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3);
        }
        format!("refs/foreversync/disk/{h:016x}")
    }

    /// Records local WTF/ changes on top of `main`.
    pub fn snapshot(&self, trigger: &str, first: FirstSync) -> Result<SnapshotOutcome> {
        let head_files = self.store.head_files()?;
        let observed = self.store.files_at_ref(&self.disk_ref())?;
        let scanned = self.observe(observed.as_ref())?;

        let mut next = head_files.clone();
        for (path, bytes) in layout::base_files() {
            next.entry(path).or_insert(bytes);
        }
        let mut overrode_pending = Vec::new();
        match &observed {
            Some(prev) => {
                for (path, bytes) in &scanned.files {
                    if prev.get(path) == Some(bytes) {
                        continue; // unchanged on disk since last look
                    }
                    let head = head_files.get(path);
                    if head.is_some() && head != prev.get(path) && head != Some(bytes) {
                        overrode_pending.push(path.clone());
                    }
                    next.insert(path.clone(), bytes.clone());
                }
                for path in prev.keys().filter(|p| !scanned.files.contains_key(*p)) {
                    next.remove(path);
                }
            }
            None => {
                for (path, bytes) in &scanned.files {
                    let take = match first {
                        FirstSync::UploadLocal => true,
                        FirstSync::UseCloud => !head_files.contains_key(path),
                    };
                    if take {
                        next.insert(path.clone(), bytes.clone());
                    }
                }
            }
        }

        let changes = Changes::between(&layout::wtf_only(&head_files), &layout::wtf_only(&next));
        let addon_changes = Changes::between(&addon_sync::addon_files(&head_files), &addon_sync::addon_files(&next));
        let message = format!(
            "Snapshot from \"{host}\" ({trigger})\n\n\
             ForeverSync-Machine: {host}\n\
             ForeverSync-Trigger: {trigger}\n\
             ForeverSync-Client: wowf {client}\n\
             ForeverSync-Version: {ver}\n",
            host = self.host,
            client = self.install.version.as_deref().unwrap_or("unknown"),
            ver = env!("CARGO_PKG_VERSION"),
        );
        let commit = self.store.snapshot(&next, &message)?.map(|id| id.to_string());
        self.store.set_files_ref(&self.disk_ref(), &scanned.files, "observed disk state")?;
        Ok(SnapshotOutcome {
            commit,
            changes,
            addon_changes,
            addons_error: scanned.addons_error,
            skipped: scanned.skipped,
            overrode_pending,
        })
    }

    /// Brings `remote_head` into `main`: fast-forward or per-file merge.
    pub fn integrate(&self, remote_head: Option<Oid>, policy: ConflictPolicy) -> Result<Integration> {
        let Some(theirs) = remote_head else { return Ok(Integration::UpToDate) };
        let Some(ours) = self.store.head()? else {
            self.store.set_head(theirs)?;
            return Ok(Integration::FastForward);
        };
        if ours == theirs {
            return Ok(Integration::UpToDate);
        }
        if self.store.is_ancestor(ours, theirs)? {
            self.store.set_head(theirs)?;
            return Ok(Integration::FastForward);
        }
        if self.store.is_ancestor(theirs, ours)? {
            return Ok(Integration::LocalAhead);
        }
        let base = match self.store.merge_base(ours, theirs)? {
            Some(b) => self.store.files_at(&b.to_string())?,
            None => FileSet::new(),
        };
        let prefer = match policy {
            ConflictPolicy::PreferLocal => Side::Local,
            ConflictPolicy::PreferRemote => Side::Remote,
            ConflictPolicy::NewestWins => {
                if self.store.commit_time(ours)? > self.store.commit_time(theirs)? {
                    Side::Local
                } else {
                    Side::Remote
                }
            }
        };
        let local = self.store.files_at(&ours.to_string())?;
        let remote = self.store.files_at(&theirs.to_string())?;
        let merged = merge3(&base, &local, &remote, prefer);
        let mut message = format!("Merge snapshots on \"{}\"\n\nForeverSync-Machine: {}\n", self.host, self.host);
        for c in &merged.conflicts {
            let kept = if c.kept == Side::Local { "local" } else { "remote" };
            message.push_str(&format!("ForeverSync-Conflict: {} (kept {kept})\n", c.path));
        }
        self.store.commit_merge(&merged.files, &message, &[ours, theirs])?;
        Ok(Integration::Merged { conflicts: merged.conflicts })
    }

    /// Writes `main` into WTF/ unless the client is running.
    pub fn apply_head(&self, force: bool) -> Result<ApplyOutcome> {
        let target = self.store.head_files()?;
        let observed = self.store.files_at_ref(&self.disk_ref())?;
        let plan = apply::plan(&self.install.wtf_dir(), &target, observed.as_ref())?;
        if plan.is_empty() {
            return Ok(ApplyOutcome::UpToDate);
        }
        if !force && process::forever_running(&self.install.flavor_dir) {
            return Ok(ApplyOutcome::GameRunning);
        }
        let report = apply::execute(&self.install.wtf_dir(), &target, &plan, &self.backup_root)?;
        let rescanned = self.observe(observed.as_ref())?;
        self.store.set_files_ref(&self.disk_ref(), &rescanned.files, "observed disk state after apply")?;
        Ok(ApplyOutcome::Applied(report))
    }

    /// Commits the WTF/ part of `rev` as a new snapshot on `main`, then
    /// applies it. Restores are ordinary commits, so they can be undone.
    pub fn restore(&self, rev: &str, force: bool) -> Result<(Option<String>, ApplyOutcome)> {
        let id = self.store.resolve(rev)?;
        let old = layout::wtf_only(&self.store.files_at(&id)?);
        let mut next: FileSet = self
            .store
            .head_files()?
            .into_iter()
            .filter(|(p, _)| !p.starts_with(layout::WTF_PREFIX))
            .collect();
        next.extend(old);
        let msg = format!(
            "Restore {} on \"{}\"\n\nForeverSync-Restore: {id}\nForeverSync-Machine: {}\nForeverSync-Trigger: restore\n",
            &id[..10],
            self.host,
            self.host
        );
        let commit = self.store.snapshot(&next, &msg)?.map(|c| c.to_string());
        Ok((commit, self.apply_head(force)?))
    }

    /// Restores only `paths` to their state at `rev` (a path missing at
    /// `rev` is removed), as a new commit on `main`, then applies it.
    pub fn restore_paths(&self, rev: &str, paths: &[String], force: bool) -> Result<(Option<String>, ApplyOutcome)> {
        let id = self.store.resolve(rev)?;
        let mut next = self.store.head_files()?;
        for path in paths {
            match self.store.file_at(&id, path)? {
                Some(bytes) => {
                    next.insert(path.clone(), bytes);
                }
                None => {
                    next.remove(path);
                }
            }
        }
        let what = if paths.len() == 1 { paths[0].clone() } else { format!("{} files", paths.len()) };
        let msg = format!(
            "Restore {what} from {} on \"{}\"\n\nForeverSync-Restore: {id}\nForeverSync-Machine: {}\nForeverSync-Trigger: restore\n",
            &id[..10],
            self.host,
            self.host
        );
        let commit = self.store.snapshot(&next, &msg)?.map(|c| c.to_string());
        Ok((commit, self.apply_head(force)?))
    }

    /// One full cycle. `remote = None` runs locally only.
    pub fn sync(
        &self,
        remote: Option<&Remote>,
        policy: ConflictPolicy,
        first: FirstSync,
        trigger: &str,
        force_apply: bool,
    ) -> Result<SyncReport> {
        let Some(remote) = remote else {
            let snapshot = self.snapshot(trigger, first)?;
            let apply = self.apply_head(force_apply)?;
            let addon_list_reset = if self.addons_source { Some(self.reset_addon_list()?) } else { None };
            return Ok(SyncReport { snapshot, integration: Integration::NoRemote, pushed: false, apply, addon_list_reset });
        };

        // A machine with no history adopts the cloud's before its first
        // snapshot, so `first` decides overlaps instead of commit times.
        let mut remote_head = self.store.fetch(remote)?;
        if self.store.head()?.is_none() {
            if let Some(theirs) = remote_head {
                self.store.set_head(theirs)?;
            }
        }
        let snapshot = self.snapshot(trigger, first)?;

        const ATTEMPTS: u32 = 3;
        let mut integration = Integration::UpToDate;
        let mut pushed = false;
        let mut addon_list_reset: Option<Changes> = None;
        for attempt in 1..=ATTEMPTS {
            let step = self.integrate(remote_head, policy)?;
            if !matches!(step, Integration::UpToDate | Integration::LocalAhead) || attempt == 1 {
                integration = step;
            }
            if self.addons_source {
                // Re-applied after every merge, so additions that arrive while
                // retrying are dropped too.
                let c = self.reset_addon_list()?;
                let acc = addon_list_reset.get_or_insert_with(Changes::default);
                acc.added.extend(c.added);
                acc.modified.extend(c.modified);
                acc.deleted.extend(c.deleted);
            }
            let head = self.store.head()?;
            if head.is_none() || head == remote_head {
                break;
            }
            match self.store.push(remote)? {
                PushOutcome::Pushed => {
                    pushed = true;
                    break;
                }
                PushOutcome::Rejected(_) if attempt < ATTEMPTS => remote_head = self.store.fetch(remote)?,
                PushOutcome::Rejected(_) => return Err(SyncError::TooManyRetries(ATTEMPTS)),
            }
        }
        let apply = self.apply_head(force_apply)?;
        Ok(SyncReport { snapshot, integration, pushed, apply, addon_list_reset })
    }
}
