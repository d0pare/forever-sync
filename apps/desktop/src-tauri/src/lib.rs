mod commands;
mod state;
mod syncer;
mod updates;
mod worker;

use foreversync_core::appdata;
use foreversync_core::sync::FirstSync;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WindowEvent};

use crate::state::AppState;

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Older Windows builds kept data inside the install folder; move it out.
    let _ = appdata::migrate_legacy_data_dir();
    let data_dir = appdata::default_data_dir().expect("no app-data folder on this OS");

    let app = tauri::Builder::default()
        // Must be first: a second launch just focuses the running app.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--background"])))
        .manage(AppState::new(data_dir))
        .setup(|app| {
            let status = MenuItem::with_id(app, "status", "Not synced yet", false, None::<&str>)?;
            let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
            let open = MenuItem::with_id(app, "open", "Open Forever Sync", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Forever Sync", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&status, &sep, &sync, &open, &sep, &quit])?;
            *app.state::<AppState>().tray_status.lock().unwrap() = Some(status);

            // macOS: a monochrome template image the menu bar tints for light
            // and dark mode. Windows: the colour app icon.
            #[cfg(target_os = "macos")]
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
            #[cfg(not(target_os = "macos"))]
            let tray_icon = app.default_window_icon().cloned().expect("bundle has an icon");

            TrayIconBuilder::with_id("main")
                .icon(tray_icon)
                .icon_as_template(cfg!(target_os = "macos"))
                .tooltip("Forever Sync")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "sync" => {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let _ = syncer::run_sync(&app, "manual", FirstSync::UseCloud, false);
                        });
                    }
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            // Started at login: stay in the tray.
            if std::env::args().any(|a| a == "--background") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            // Start-at-login stores the app's path when enabled; refresh it so
            // moving or updating the app keeps the login item pointing here.
            {
                use tauri_plugin_autostart::ManagerExt;
                let al = app.autolaunch();
                if al.is_enabled().unwrap_or(false) {
                    let _ = al.enable();
                }
            }
            // Empty addon trash older than 30 days.
            if let Ok(install) = foreversync_core::discovery::discover(None) {
                let _ = foreversync_core::addon_cleanup::purge_trash(
                    &install.addons_dir(),
                    foreversync_core::addon_cleanup::TRASH_KEEP,
                );
            }
            worker::spawn(app.handle().clone());
            updates::spawn(app.handle().clone());
            Ok(())
        })
        // Closing the window keeps syncing in the tray; quit from the tray menu.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_state,
            commands::start_login,
            commands::finish_login,
            commands::logout,
            commands::find_repo,
            commands::choose_repo,
            commands::sync_now,
            commands::set_autostart,
            commands::addons,
            commands::status,
            commands::addon_removal_plan,
            commands::remove_addons,
            commands::check_update,
            commands::install_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app, _event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            show_main(_app);
        }
    });
}
