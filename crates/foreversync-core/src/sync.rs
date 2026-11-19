//! The sync cycle (DESIGN.md §7.2): fetch → snapshot → merge → push → apply.
//!
//! Local changes are detected against the **last observed disk state**
//! (kept under a local-only ref per install), not against `main`. So:
//! * files in `main` that this machine hasn't applied yet (game running) are
//!   never reverted or "deleted" by a snapshot;
//! * a character folder that exists only on another machine is never treated
//!   as deleted here.

use std::path::{Path, PathBuf};

use foreversync_git::{Oid, PushOutcome, Remote, Store, StoreError};
use serde::Serialize;

use crate::merge::{Conflict, Side, merge3};
use crate::{Changes, FileSet, ForeverInstall, apply, layout, process, scan};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("the remote kept moving; gave up after {0} attempts")]
    TooManyRetries(u32),
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
    pub changes: Changes,
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
}

pub struct Engine<'a> {
    pub install: &'a ForeverInstall,
    pub store: &'a Store,
    pub backup_root: PathBuf,
    pub host: String,
}

impl<'a> Engine<'a> {
    pub fn new(install: &'a ForeverInstall, store: &'a Store, backup_root: &Path) -> Self {
        Self { install, store, backup_root: backup_root.to_path_buf(), host: process::host_name() }
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
        let scanned = scan::scan_wtf(&self.install.wtf_dir())?;
        let head_files = self.store.head_files()?;
        let observed = self.store.files_at_ref(&self.disk_ref())?;

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
        Ok(SnapshotOutcome { commit, changes, skipped: scanned.skipped, overrode_pending })
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
        let mut message = format!("Merge snapshots on \"{}\"\n\n", self.host);
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
        let rescanned = scan::scan_wtf(&self.install.wtf_dir())?;
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
        let msg = format!("Restore {} on \"{}\"\n\nForeverSync-Restore: {id}\n", &id[..10], self.host);
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
            return Ok(SyncReport { snapshot, integration: Integration::NoRemote, pushed: false, apply });
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
        for attempt in 1..=ATTEMPTS {
            let step = self.integrate(remote_head, policy)?;
            if !matches!(step, Integration::UpToDate | Integration::LocalAhead) || attempt == 1 {
                integration = step;
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
        Ok(SyncReport { snapshot, integration, pushed, apply })
    }
}
