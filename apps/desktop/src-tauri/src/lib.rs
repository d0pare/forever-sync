use std::time::{SystemTime, UNIX_EPOCH};

use foreversync_addons::{ImportPayload, Inventory, default_wowup_dirs, import_string, load_inventory};
use foreversync_core::{ForeverInstall, discovery};
use serde::Serialize;

#[derive(Serialize)]
struct Overview {
    install: ForeverInstall,
    /// `None` with `addons_error` set when WowUp data can't be read; settings
    /// sync still works in that case (DESIGN.md §11, S3).
    addons: Option<Inventory>,
    addons_error: Option<String>,
    import_string: Option<String>,
}

/// Read-only look at the local Forever install and its WowUp addon list.
#[tauri::command]
fn overview() -> Result<Overview, String> {
    let install = discovery::discover(None).map_err(|e| e.to_string())?;
    let (addons, addons_error, import) =
        match load_inventory(&default_wowup_dirs(), &install.flavor_dir, &install.addons_dir()) {
            Ok(inv) => {
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                let s = import_string(&ImportPayload::from_addons(&inv.managed, now_ms));
                (Some(inv), None, Some(s))
            }
            Err(e) => (None, Some(e.to_string()), None),
        };
    Ok(Overview { install, addons, addons_error, import_string: import })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![overview])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
