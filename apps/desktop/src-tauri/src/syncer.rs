//! Runs one sync cycle and publishes the result to the UI and tray.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use foreversync_core::sync::{ApplyOutcome, ConflictPolicy, Engine, FirstSync, Integration};
use foreversync_core::{appdata, discovery};
use foreversync_git::{Remote, Store};
use foreversync_github::{self as gh, KeychainStore};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{AppState, LastSync, Status};

const PRIVACY_RECHECK: Duration = Duration::from_secs(60 * 60);

/// Runs a full sync. Returns `Ok(None)` if another sync is already running.
pub fn run_sync(app: &AppHandle, trigger: &str, first: FirstSync) -> Result<Option<LastSync>, String> {
    let state = app.state::<AppState>();
    let Ok(_guard) = state.sync_lock.try_lock() else { return Ok(None) };
    update_status(app, |s| s.syncing = true);

    let result = sync_inner(&state, trigger, first);
    update_status(app, |s| {
        s.syncing = false;
        match &result {
            Ok(last) => {
                s.last_sync = Some(last.clone());
                s.last_error = None;
            }
            Err(e) => s.last_error = Some(e.clone()),
        }
    });
    result.map(Some)
}

fn sync_inner(state: &AppState, trigger: &str, first: FirstSync) -> Result<LastSync, String> {
    let cfg = appdata::load_config(&state.data_dir)
        .map_err(|e| e.to_string())?
        .ok_or("Choose a GitHub repo first.")?;
    let token = gh::valid_access_token(&KeychainStore::default()).map_err(|e| match e {
        gh::GithubError::NotSignedIn => "Sign in to GitHub again.".to_owned(),
        e => e.to_string(),
    })?;

    // Privacy guard (DESIGN.md §8.3): never push to a public repo.
    let fresh = matches!(&*state.privacy_checked.lock().unwrap(), Some((r, t)) if *r == cfg.repo && t.elapsed() < PRIVACY_RECHECK);
    if !fresh {
        let info = gh::Client::new(token.clone()).repo(&cfg.repo).map_err(|e| e.to_string())?;
        if !info.private {
            return Err(format!("{} is public. Make it private on GitHub; Forever Sync won't sync to a public repo.", cfg.repo));
        }
        *state.privacy_checked.lock().unwrap() = Some((cfg.repo.clone(), Instant::now()));
    }

    let install = discovery::discover(None).map_err(|e| e.to_string())?;
    let store = Store::open_or_init(&appdata::repo_dir(&state.data_dir)).map_err(|e| e.to_string())?;
    let engine = Engine::new(&install, &store, &appdata::backups_dir(&state.data_dir));
    let remote = Remote { url: cfg.clone_url, token: Some(token) };
    let report = engine
        .sync(Some(&remote), ConflictPolicy::NewestWins, first, trigger, false)
        .map_err(|e| e.to_string())?;

    let changes = &report.snapshot.changes;
    let (written, deleted, pending_apply) = match &report.apply {
        ApplyOutcome::Applied(r) => (r.written, r.deleted, false),
        ApplyOutcome::GameRunning => (0, 0, true),
        ApplyOutcome::UpToDate => (0, 0, false),
    };
    Ok(LastSync {
        at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
        trigger: trigger.to_owned(),
        local_changes: changes.added.len() + changes.modified.len() + changes.deleted.len(),
        took_remote: matches!(report.integration, Integration::FastForward | Integration::Merged { .. }),
        conflicts: match &report.integration {
            Integration::Merged { conflicts } => conflicts.iter().map(|c| c.path.clone()).collect(),
            _ => Vec::new(),
        },
        pushed: report.pushed,
        written,
        deleted,
        pending_apply,
        skipped: report.snapshot.skipped.iter().map(|s| format!("{}: {}", s.path, s.reason)).collect(),
    })
}

/// Mutates the shared status, then tells the UI and refreshes the tray text.
pub fn update_status(app: &AppHandle, f: impl FnOnce(&mut Status)) {
    let state = app.state::<AppState>();
    let snapshot = {
        let mut s = state.status.lock().unwrap();
        f(&mut s);
        s.clone()
    };
    let _ = app.emit("status", &snapshot);
    if let Some(item) = state.tray_status.lock().unwrap().as_ref() {
        let _ = item.set_text(tray_text(&snapshot));
    }
}

pub fn tray_text(s: &Status) -> String {
    if s.syncing {
        "Syncing…".into()
    } else if s.last_error.is_some() {
        "Sync problem: open Forever Sync".into()
    } else if s.last_sync.as_ref().is_some_and(|l| l.pending_apply) {
        "Changes wait until you quit WoW".into()
    } else if s.last_sync.is_some() {
        "Up to date".into()
    } else {
        "Not synced yet".into()
    }
}
