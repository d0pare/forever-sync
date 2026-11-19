//! Commands the UI calls. Blocking work (network, git, disk) runs on the
//! blocking thread pool so the UI stays responsive.

use std::time::{SystemTime, UNIX_EPOCH};

use foreversync_addons::{ImportPayload, Inventory, default_wowup_dirs, import_string, load_inventory};
use foreversync_core::addon_cleanup::{self, RemovalPlan, RemovalReport};
use foreversync_core::addon_sync::{self, AddonStatus};
use foreversync_core::process;
use foreversync_core::appdata::{self, RepoConfig};
use foreversync_core::sync::FirstSync;
use foreversync_core::{ForeverInstall, discovery};
use foreversync_github::{self as gh, KeychainStore, TokenStore};
use serde::Serialize;
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

#[derive(Serialize)]
pub struct AddonsView {
    inventory: Option<Inventory>,
    error: Option<String>,
    /// Import string for everything installed here.
    import_string: Option<String>,
    /// This machine vs the synced list (from the last sync; no network).
    status: Option<AddonStatus>,
    /// Number of addons in the synced list.
    synced_count: usize,
}

/// WowUp addon list, how it compares with the synced list, import strings.
#[tauri::command]
pub async fn addons(app: AppHandle) -> CmdResult<AddonsView> {
    blocking(move || {
        let install = discovery::discover(None).map_err(|e| e.to_string())?;
        let data_dir = app.state::<AppState>().data_dir.clone();
        let synced = foreversync_git::Store::open_or_init(&appdata::repo_dir(&data_dir))
            .and_then(|s| s.head_files())
            .unwrap_or_default();
        let synced_count = addon_sync::packages_in(&synced).len();
        Ok(match load_inventory(&default_wowup_dirs(), &install.flavor_dir, &install.addons_dir()) {
            Ok(inv) => {
                let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                let s = import_string(&ImportPayload::from_addons(&inv.managed, now_ms));
                let status = (synced_count > 0).then(|| addon_sync::status(&synced, &inv));
                AddonsView { inventory: Some(inv), error: None, import_string: Some(s), status, synced_count }
            }
            Err(e) => AddonsView { inventory: None, error: Some(e.to_string()), import_string: None, status: None, synced_count },
        })
    })
    .await
}

#[tauri::command]
pub fn status(state: State<'_, AppState>) -> Status {
    state.status.lock().unwrap().clone()
}

fn synced_and_local(app: &AppHandle) -> CmdResult<(ForeverInstall, foreversync_core::FileSet, Inventory)> {
    let install = discovery::discover(None).map_err(|e| e.to_string())?;
    let data_dir = app.state::<AppState>().data_dir.clone();
    let synced = foreversync_git::Store::open_or_init(&appdata::repo_dir(&data_dir))
        .and_then(|s| s.head_files())
        .map_err(|e| e.to_string())?;
    let inv = load_inventory(&default_wowup_dirs(), &install.flavor_dir, &install.addons_dir()).map_err(|e| e.to_string())?;
    Ok((install, synced, inv))
}

/// What "remove addons uninstalled on other computers" would move to trash.
#[tauri::command]
pub async fn addon_removal_plan(app: AppHandle) -> CmdResult<RemovalPlan> {
    blocking(move || {
        let (_, synced, inv) = synced_and_local(&app)?;
        Ok(addon_cleanup::plan(&synced, &inv, None))
    })
    .await
}

/// Moves addons uninstalled on other computers to the AddOns trash.
#[tauri::command]
pub async fn remove_addons(app: AppHandle) -> CmdResult<RemovalReport> {
    blocking(move || {
        let (install, synced, inv) = synced_and_local(&app)?;
        if process::forever_running(&install.flavor_dir) {
            return Err("Quit WoW Forever first; addons can't be removed while the game is running.".into());
        }
        let plan = addon_cleanup::plan(&synced, &inv, None);
        addon_cleanup::execute(&install.addons_dir(), &plan).map_err(|e| e.to_string())
    })
    .await
}
