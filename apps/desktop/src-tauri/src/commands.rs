//! Commands the UI calls. Blocking work (network, git, disk) runs on the
//! blocking thread pool so the UI stays responsive.

use std::time::{SystemTime, UNIX_EPOCH};

use foreversync_addons::{ImportPayload, Inventory, default_wowup_dirs, import_string, load_inventory};
use foreversync_core::addon_cleanup::{self, RemovalPlan, RemovalReport};
use foreversync_core::addon_sync::{self, AddonRow, AddonStatus, PackageFile};
use foreversync_core::process;
use foreversync_core::appdata::{self, RepoConfig};
use foreversync_core::sync::{Engine, FirstSync, disk_ref};
use foreversync_core::{ForeverInstall, discovery};
use foreversync_github::{self as gh, KeychainStore, TokenStore};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::state::{AppState, DesktopSettings, LastSync, Status};
use crate::syncer::run_sync;

type CmdResult<T> = Result<T, String>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct AppStateView {
    signed_in: bool,
    user: Option<String>,
    repo: Option<RepoConfig>,
    onboarded: bool,
    autostart: bool,
    install: Option<ForeverInstall>,
    install_error: Option<String>,
    status: Status,
    data_dir: String,
    version: String,
    update: Option<crate::updates::UpdateInfo>,
}

#[tauri::command]
pub async fn app_state(app: AppHandle) -> CmdResult<AppStateView> {
    blocking(move || {
        let state = app.state::<AppState>();
        let signed_in = KeychainStore::default().load().map_err(|e| e.to_string())?.is_some();
        let mut user = state.user.lock().unwrap().clone();
        if signed_in && user.is_none() {
            if let Ok(token) = gh::valid_access_token(&KeychainStore::default()) {
                user = gh::Client::new(token).user().ok().map(|u| u.login);
                *state.user.lock().unwrap() = user.clone();
            }
        }
        let (install, install_error) = match discovery::discover(None) {
            Ok(i) => (Some(i), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Ok(AppStateView {
            signed_in,
            user,
            repo: appdata::load_config(&state.data_dir).map_err(|e| e.to_string())?,
            onboarded: DesktopSettings::load(&state.data_dir).onboarded,
            autostart: app.autolaunch().is_enabled().unwrap_or(false),
            install,
            install_error,
            status: state.status.lock().unwrap().clone(),
            data_dir: state.data_dir.display().to_string(),
            version: app.package_info().version.to_string(),
            update: state.update.lock().unwrap().clone(),
        })
    })
    .await
}

#[derive(Serialize)]
pub struct LoginStart {
    user_code: String,
    verification_uri: String,
    expires_in: u64,
}

/// Step 1 of sign-in: get a device code to show the user.
#[tauri::command]
pub async fn start_login(app: AppHandle) -> CmdResult<LoginStart> {
    blocking(move || {
        let code = gh::start_device_flow().map_err(|e| e.to_string())?;
        let view = LoginStart {
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
            expires_in: code.expires_in,
        };
        *app.state::<AppState>().pending_login.lock().unwrap() = Some(code);
        Ok(view)
    })
    .await
}

/// Step 2 of sign-in: wait for approval, store tokens in the keychain.
#[tauri::command]
pub async fn finish_login(app: AppHandle) -> CmdResult<String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let code = state.pending_login.lock().unwrap().take().ok_or("Start sign-in first.")?;
        let tokens = gh::poll_device_flow(&code).map_err(|e| e.to_string())?;
        KeychainStore::default().save(&tokens).map_err(|e| e.to_string())?;
        let login = gh::Client::new(tokens.access_token).user().map_err(|e| e.to_string())?.login;
        *state.user.lock().unwrap() = Some(login.clone());
        Ok(login)
    })
    .await
}

#[tauri::command]
pub async fn logout(app: AppHandle) -> CmdResult<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        KeychainStore::default().clear().map_err(|e| e.to_string())?;
        *state.user.lock().unwrap() = None;
        *state.privacy_checked.lock().unwrap() = None;
        Ok(())
    })
    .await
}

#[derive(Serialize)]
pub struct RepoChoice {
    full_name: String,
    private: bool,
    /// Already holds Forever Sync data (this is an additional machine).
    initialized: bool,
}

#[derive(Serialize)]
pub struct RepoSearch {
    found: Option<RepoChoice>,
    new_repo_url: String,
    install_url: String,
}

/// Looks for the data repo among repos the app is installed on.
#[tauri::command]
pub async fn find_repo() -> CmdResult<RepoSearch> {
    blocking(|| {
        let client = gh::Client::new(gh::valid_access_token(&KeychainStore::default()).map_err(|e| e.to_string())?);
        let found = client.find_data_repo().map_err(|e| e.to_string())?.map(|d| RepoChoice {
            full_name: d.repo.full_name,
            private: d.repo.private,
            initialized: d.initialized,
        });
        Ok(RepoSearch { found, new_repo_url: gh::config::new_repo_url(), install_url: gh::config::install_url() })
    })
    .await
}

/// Saves `full_name` as this machine's data repo (must be private).
#[tauri::command]
pub async fn choose_repo(app: AppHandle, full_name: String) -> CmdResult<RepoChoice> {
    blocking(move || {
        let client = gh::Client::new(gh::valid_access_token(&KeychainStore::default()).map_err(|e| e.to_string())?);
        let info = client.repo(&full_name).map_err(|e| e.to_string())?;
        if !info.private {
            return Err(format!("{full_name} is public. Make it private first: SavedVariables can contain chat and character data."));
        }
        let initialized = client.has_file(&full_name, "foreversync.json").map_err(|e| e.to_string())?;
        let data_dir = app.state::<AppState>().data_dir.clone();
        appdata::save_config(&data_dir, &RepoConfig { repo: info.full_name.clone(), clone_url: info.clone_url })
            .map_err(|e| e.to_string())?;
        Ok(RepoChoice { full_name: info.full_name, private: true, initialized })
    })
    .await
}

/// What each first-sync choice would change, before choosing (fetches, but
/// commits and writes nothing).
#[tauri::command]
pub async fn first_sync_preview(app: AppHandle) -> CmdResult<foreversync_core::preview::FirstSyncPreview> {
    blocking(move || {
        let state = app.state::<AppState>();
        let _guard = state.sync_lock.lock().map_err(|e| e.to_string())?;
        let ctx = crate::syncer::open_context(&state)?;
        let engine = foreversync_core::sync::Engine::new(&ctx.install, &ctx.store, &appdata::backups_dir(&state.data_dir));
        engine.preview_first_sync(Some(&ctx.remote)).map_err(|e| e.to_string())
    })
    .await
}

/// Runs a sync now. `first_sync` ("use-cloud" | "upload-local") only matters
/// on this machine's first sync; the first successful one completes onboarding.
#[tauri::command]
pub async fn sync_now(app: AppHandle, first_sync: Option<String>, addons_source: Option<bool>) -> CmdResult<Option<LastSync>> {
    blocking(move || {
        let first = match first_sync.as_deref() {
            Some("upload-local") => FirstSync::UploadLocal,
            _ => FirstSync::UseCloud,
        };
        let trigger = if addons_source == Some(true) { "addons-source" } else { "manual" };
        let result = run_sync(&app, trigger, first, addons_source == Some(true))?;
        let data_dir = app.state::<AppState>().data_dir.clone();
        let mut settings = DesktopSettings::load(&data_dir);
        if result.is_some() && !settings.onboarded {
            settings.onboarded = true;
            settings.save(&data_dir).map_err(|e| e.to_string())?;
            // Start at login by default once set up (release builds only, so dev
            // builds don't register target/debug binaries as login items).
            if !cfg!(debug_assertions) {
                let _ = app.autolaunch().enable();
            }
        }
        Ok(result)
    })
    .await
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> CmdResult<bool> {
    let al = app.autolaunch();
    if enabled { al.enable() } else { al.disable() }.map_err(|e| e.to_string())?;
    al.is_enabled().map_err(|e| e.to_string())
}

/// One addon to act on, by WowUp provider and id.
#[derive(Debug, Clone, Deserialize)]
pub struct AddonKey {
    provider: String,
    id: String,
}

#[derive(Serialize)]
pub struct TrashInfo {
    dir: String,
    batches: usize,
}

#[derive(Serialize)]
pub struct AddonsView {
    /// Why WowUp couldn't be read, if it couldn't.
    error: Option<String>,
    wowup_dir: Option<String>,
    /// Every addon in the synced list or installed here.
    rows: Vec<AddonRow>,
    status: Option<AddonStatus>,
    /// Folders in AddOns that WowUp doesn't manage (not synced).
    unmanaged_folders: Vec<String>,
    /// Import string for everything installed here.
    import_string: Option<String>,
    synced_count: usize,
    trash: Option<TrashInfo>,
    game_running: bool,
}

/// The synced list (local `main`, as of the last sync), this machine's
/// observed list at that sync, and the WowUp inventory.
struct AddonContext {
    install: ForeverInstall,
    store: foreversync_git::Store,
    synced: foreversync_core::FileSet,
    observed: Option<foreversync_core::FileSet>,
    inventory: Result<Inventory, String>,
}

fn addon_context(app: &AppHandle) -> CmdResult<AddonContext> {
    let install = discovery::discover(None).map_err(|e| e.to_string())?;
    let data_dir = app.state::<AppState>().data_dir.clone();
    let store = foreversync_git::Store::open_or_init(&appdata::repo_dir(&data_dir)).map_err(|e| e.to_string())?;
    let synced = store.head_files().map_err(|e| e.to_string())?;
    let observed = store.files_at_ref(&disk_ref(&install.flavor_dir)).map_err(|e| e.to_string())?;
    let inventory =
        load_inventory(&default_wowup_dirs(), &install.flavor_dir, &install.addons_dir()).map_err(|e| e.to_string());
    Ok(AddonContext { install, store, synced, observed, inventory })
}

/// Keeps the dashboard and tray addon counts current outside of syncs.
fn refresh_addon_counts(app: &AppHandle, status: &AddonStatus) {
    crate::syncer::update_status(app, |s| {
        if let Some(last) = s.last_sync.as_mut() {
            last.addons_missing = status.missing.len();
            last.addons_removed_elsewhere = status.removed_elsewhere.len();
        }
    });
}

/// Everything the Addons tab shows.
#[tauri::command]
pub async fn addons(app: AppHandle) -> CmdResult<AddonsView> {
    blocking(move || {
        let ctx = addon_context(&app)?;
        let synced_count = addon_sync::packages_in(&ctx.synced).len();
        let trash_root = ctx.install.addons_dir().parent().map(|p| p.join(addon_cleanup::TRASH_DIR));
        let trash = trash_root.and_then(|dir| {
            let batches = std::fs::read_dir(&dir).ok()?.filter_map(Result::ok).filter(|e| e.path().is_dir()).count();
            (batches > 0).then(|| TrashInfo { dir: dir.display().to_string(), batches })
        });
        let game_running = process::forever_running(&ctx.install.flavor_dir);
        Ok(match ctx.inventory {
            Ok(inv) => {
                let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                let import = import_string(&ImportPayload::from_addons(&inv.managed, now_ms));
                let status = addon_sync::status(&ctx.synced, ctx.observed.as_ref(), &inv);
                refresh_addon_counts(&app, &status);
                AddonsView {
                    error: None,
                    wowup_dir: Some(inv.wowup_dir.display().to_string()),
                    rows: addon_sync::overview(&ctx.synced, ctx.observed.as_ref(), &inv),
                    status: Some(status),
                    unmanaged_folders: inv.unmanaged_folders.clone(),
                    import_string: Some(import),
                    synced_count,
                    trash,
                    game_running,
                }
            }
            Err(e) => AddonsView {
                error: Some(e),
                wowup_dir: None,
                rows: Vec::new(),
                status: None,
                unmanaged_folders: Vec::new(),
                import_string: None,
                synced_count,
                trash,
                game_running,
            },
        })
    })
    .await
}

#[tauri::command]
pub fn status(state: State<'_, AppState>) -> Status {
    state.status.lock().unwrap().clone()
}

fn only_keys(keys: &Option<Vec<AddonKey>>) -> Option<Vec<(String, String)>> {
    keys.as_ref().map(|v| v.iter().map(|k| (k.provider.clone(), k.id.clone())).collect())
}

/// What removing addons uninstalled on other computers would move to trash
/// (all of them, or just `keys`).
#[tauri::command]
pub async fn addon_removal_plan(app: AppHandle, keys: Option<Vec<AddonKey>>) -> CmdResult<RemovalPlan> {
    blocking(move || {
        let ctx = addon_context(&app)?;
        let inv = ctx.inventory?;
        Ok(addon_cleanup::plan(&ctx.synced, ctx.observed.as_ref(), &inv, only_keys(&keys).as_deref()))
    })
    .await
}

/// Moves addons uninstalled on other computers to the AddOns trash and drops
/// their WowUp records (plus any WowUp leftovers). `keys: []` clears only the
/// leftovers.
#[tauri::command]
pub async fn remove_addons(app: AppHandle, keys: Option<Vec<AddonKey>>) -> CmdResult<RemovalReport> {
    blocking(move || {
        let state = app.state::<AppState>();
        // No sync may record the observed state halfway through.
        let _guard = state.sync_lock.lock().map_err(|e| e.to_string())?;
        let ctx = addon_context(&app)?;
        if process::forever_running(&ctx.install.flavor_dir) {
            return Err("Quit WoW Forever first; addons can't be removed while the game is running.".into());
        }
        let inv = ctx.inventory?;
        let plan = addon_cleanup::plan(&ctx.synced, ctx.observed.as_ref(), &inv, only_keys(&keys).as_deref());
        let report = addon_cleanup::execute(&ctx.install.addons_dir(), &inv, &plan).map_err(|e| e.to_string())?;
        // Forget them here, so a later reinstall counts as new.
        let engine = Engine::new(&ctx.install, &ctx.store, &appdata::backups_dir(&state.data_dir));
        engine.forget_addons(&plan.packages).map_err(|e| e.to_string())?;
        if let Ok(inv) = load_inventory(&default_wowup_dirs(), &ctx.install.flavor_dir, &ctx.install.addons_dir()) {
            let observed = ctx.store.files_at_ref(&engine.disk_ref()).ok().flatten();
            refresh_addon_counts(&app, &addon_sync::status(&ctx.synced, observed.as_ref(), &inv));
        }
        Ok(report)
    })
    .await
}

/// Keeps addons that were uninstalled on another computer: they go back on
/// the synced list, so the other computers are asked to install them again.
#[tauri::command]
pub async fn keep_addons(app: AppHandle, keys: Vec<AddonKey>) -> CmdResult<Option<LastSync>> {
    blocking(move || {
        {
            let state = app.state::<AppState>();
            let _guard = state.sync_lock.lock().map_err(|e| e.to_string())?;
            let ctx = addon_context(&app)?;
            let inv = ctx.inventory?;
            let status = addon_sync::status(&ctx.synced, ctx.observed.as_ref(), &inv);
            let keep: Vec<PackageFile> = status
                .removed_elsewhere
                .into_iter()
                .filter(|p| keys.iter().any(|k| k.provider == p.provider && k.id == p.id))
                .collect();
            let engine = Engine::new(&ctx.install, &ctx.store, &appdata::backups_dir(&state.data_dir));
            engine.forget_addons(&keep).map_err(|e| e.to_string())?;
        }
        // The next snapshot sees them as newly installed and adds them back.
        run_sync(&app, "keep-addons", FirstSync::UseCloud, false)
    })
    .await
}

/// Opens the folder holding removed addons (kept 30 days).
#[tauri::command]
pub async fn open_addon_trash(app: AppHandle) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let install = discovery::discover(None).map_err(|e| e.to_string())?;
    let dir = install.addons_dir().parent().map(|p| p.join(addon_cleanup::TRASH_DIR)).ok_or("no Interface folder")?;
    app.opener().open_path(dir.display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> CmdResult<Option<crate::updates::UpdateInfo>> {
    crate::updates::check(&app).await
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> CmdResult<()> {
    crate::updates::install(&app).await
}

// ------------------------------------------------------------------ history

#[tauri::command]
pub async fn history(app: AppHandle, limit: Option<usize>) -> CmdResult<Vec<foreversync_core::history::HistoryEntry>> {
    blocking(move || {
        let data_dir = app.state::<AppState>().data_dir.clone();
        let store = foreversync_git::Store::open_or_init(&appdata::repo_dir(&data_dir)).map_err(|e| e.to_string())?;
        foreversync_core::history::list(&store, limit.unwrap_or(50)).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn history_details(app: AppHandle, id: String) -> CmdResult<foreversync_core::history::HistoryDetails> {
    blocking(move || {
        let data_dir = app.state::<AppState>().data_dir.clone();
        let store = foreversync_git::Store::open_or_init(&appdata::repo_dir(&data_dir)).map_err(|e| e.to_string())?;
        foreversync_core::history::details(&store, &id).map_err(|e| e.to_string())
    })
    .await
}

/// What to restore from a snapshot.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RestoreScope {
    /// All settings (WTF) as of the snapshot.
    All,
    /// One addon's SavedVariables across all accounts and characters.
    Addon { addon: String },
    /// Specific files.
    Paths { paths: Vec<String> },
    /// For a conflict in merge `rev`: the version that wasn't kept.
    Other { path: String, kept: String },
}

#[derive(Serialize)]
pub struct RestoreResult {
    /// New commit, or None if nothing differed.
    commit: Option<String>,
    files: usize,
    /// "applied", "game-running" or "up-to-date".
    applied: String,
    backup_dir: Option<String>,
    pushed: bool,
    sync_error: Option<String>,
}

/// Restores settings from snapshot `rev` as a new commit, applies it (unless
/// the game runs), then syncs so other computers get it too.
#[tauri::command]
pub async fn restore(app: AppHandle, rev: String, scope: RestoreScope) -> CmdResult<RestoreResult> {
    blocking(move || {
        use foreversync_core::history;
        use foreversync_core::sync::{ApplyOutcome, Engine, FirstSync};

        let state = app.state::<AppState>();
        let (commit, files, applied) = {
            let Ok(_guard) = state.sync_lock.try_lock() else {
                return Err("A sync is running; try again in a moment.".into());
            };
            let install = discovery::discover(None).map_err(|e| e.to_string())?;
            let store =
                foreversync_git::Store::open_or_init(&appdata::repo_dir(&state.data_dir)).map_err(|e| e.to_string())?;
            let engine = Engine::new(&install, &store, &appdata::backups_dir(&state.data_dir));
            let (rev, paths): (String, Option<Vec<String>>) = match scope {
                RestoreScope::All => (rev, None),
                RestoreScope::Paths { paths } => (rev, Some(paths)),
                RestoreScope::Addon { addon } => {
                    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
                    set.extend(history::addon_settings_paths(&store.files_at(&rev).map_err(|e| e.to_string())?, &addon));
                    set.extend(history::addon_settings_paths(&store.head_files().map_err(|e| e.to_string())?, &addon));
                    (rev, Some(set.into_iter().collect()))
                }
                RestoreScope::Other { path, kept } => {
                    let entry = history::list(&store, 500)
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .find(|e| e.id == rev)
                        .ok_or("That merge is too old to look up.")?;
                    let note = history::ConflictNote { path: path.clone(), kept };
                    let other = history::other_side(&entry, &note).ok_or("Not a merge.")?.to_owned();
                    (other, Some(vec![path]))
                }
            };
            let (commit, outcome, files) = match paths {
                None => {
                    let (c, o) = engine.restore(&rev, false).map_err(|e| e.to_string())?;
                    (c, o, 0)
                }
                Some(p) if p.is_empty() => return Err("Nothing to restore.".into()),
                Some(p) => {
                    let n = p.len();
                    let (c, o) = engine.restore_paths(&rev, &p, false).map_err(|e| e.to_string())?;
                    (c, o, n)
                }
            };
            (commit, files, outcome)
        };
        let (applied, backup_dir) = match applied {
            ApplyOutcome::Applied(r) => ("applied", r.backup_dir.map(|p| p.display().to_string())),
            ApplyOutcome::GameRunning => ("game-running", None),
            ApplyOutcome::UpToDate => ("up-to-date", None),
        };
        // Share the restore with other computers right away.
        let (pushed, sync_error) = match run_sync(&app, "restore", FirstSync::UseCloud, false) {
            Ok(Some(last)) => (last.pushed, None),
            Ok(None) => (false, None),
            Err(e) => (false, Some(e)),
        };
        Ok(RestoreResult { commit, files, applied: applied.into(), backup_dir, pushed, sync_error })
    })
    .await
}
