//! Read-only access to WowUp's `preferences.json` and `addons.json`.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// WowUp's `WowClientType.Forever`.
pub const FOREVER_CLIENT_TYPE: i64 = 10;

#[derive(Debug, thiserror::Error)]
pub enum WowUpError {
    #[error("WowUp data not found (looked in: {0})")]
    NotFound(String),
    #[error("WowUp has no WoW Forever installation configured")]
    NoForeverInstallation,
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// One addon package WowUp manages for the Forever installation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManagedAddon {
    pub name: String,
    /// WowUp provider name, e.g. `Curse`, `WowUpHub`, `GitHub`.
    pub provider: String,
    pub external_id: String,
    pub version_id: Option<String>,
    pub version: Option<String>,
    pub folders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Inventory {
    /// The WowUp data folder that was read.
    pub wowup_dir: PathBuf,
    /// WowUp's id for the Forever installation.
    pub installation_id: String,
    /// Addons WowUp manages, sorted by name.
    pub managed: Vec<ManagedAddon>,
    /// Folders in `Interface/AddOns` that no WowUp record covers. Not synced.
    pub unmanaged_folders: Vec<String>,
    /// Records dropped as duplicates or because their folders are gone.
    pub dropped_records: usize,
}

/// WowUp data folders to try, in order: WowUp-CF first, then plain WowUp.
/// macOS: `~/Library/Application Support/<name>`; Windows: `%APPDATA%\<name>`.
pub fn default_wowup_dirs() -> Vec<PathBuf> {
    dirs::config_dir()
        .map(|base| vec![base.join("WowUpCf"), base.join("WowUp")])
        .unwrap_or_default()
}

/// Builds the addon inventory for the Forever install.
///
/// * `wowup_dirs` – candidate WowUp data folders (see [`default_wowup_dirs`]).
/// * `flavor_dir` – the `_forever_` folder, used to pick the right WowUp
///   installation when several Forever ones exist.
/// * `addons_dir` – `_forever_/Interface/AddOns`, listed (never modified) to
///   drop stale records and find unmanaged folders.
pub fn load_inventory(
    wowup_dirs: &[PathBuf],
    flavor_dir: &Path,
    addons_dir: &Path,
) -> Result<Inventory, WowUpError> {
    let wowup_dir = wowup_dirs
        .iter()
        .find(|d| d.join("addons.json").is_file() && d.join("preferences.json").is_file())
        .cloned()
        .ok_or_else(|| {
            WowUpError::NotFound(
                wowup_dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", "),
            )
        })?;

    let prefs: Preferences = read_json(&wowup_dir.join("preferences.json"))?;
    let installation_id =
        pick_installation(&prefs, flavor_dir).ok_or(WowUpError::NoForeverInstallation)?;

    let records: HashMap<String, AddonRecord> = read_json(&wowup_dir.join("addons.json"))?;
    let on_disk = list_folders(addons_dir)?;

    let (managed, dropped_records) = select_managed(records.into_values(), &installation_id, &on_disk);

    let covered: BTreeSet<&str> =
        managed.iter().flat_map(|a| a.folders.iter().map(String::as_str)).collect();
    let unmanaged_folders = on_disk.iter().filter(|f| !covered.contains(f.as_str())).cloned().collect();

    Ok(Inventory { wowup_dir, installation_id, managed, unmanaged_folders, dropped_records })
}

// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Preferences {
    #[serde(default)]
    wow_installations: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Installation {
    id: String,
    client_type: i64,
    #[serde(default)]
    location: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddonRecord {
    #[serde(default)]
    name: String,
    #[serde(default)]
    installation_id: String,
    #[serde(default)]
    provider_name: String,
    #[serde(default)]
    external_id: Option<String>,
    #[serde(default)]
    installed_external_release_id: Option<String>,
    #[serde(default)]
    installed_version: Option<String>,
    #[serde(default)]
    installed_folders: Option<String>,
    #[serde(default)]
    installed_folder_list: Vec<String>,
    #[serde(default)]
    installed_at: Option<String>,
    /// WowUp marks addons it couldn't match as ignored; those aren't exported.
    #[serde(default = "default_true")]
    is_ignored: bool,
}

fn default_true() -> bool {
    true
}

impl AddonRecord {
    fn folders(&self) -> Vec<String> {
        let list: Vec<String> = if self.installed_folder_list.is_empty() {
            self.installed_folders
                .as_deref()
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_owned())
                .collect()
        } else {
            self.installed_folder_list.iter().map(|s| s.trim().to_owned()).collect()
        };
        list.into_iter().filter(|s| !s.is_empty()).collect()
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, WowUpError> {
    let text = fs::read_to_string(path).map_err(|source| WowUpError::Io { path: path.into(), source })?;
    serde_json::from_str(&text).map_err(|source| WowUpError::Parse { path: path.into(), source })
}

/// `wow_installations` is a JSON array; older WowUp versions stored it as a
/// JSON-encoded string, so accept both.
fn pick_installation(prefs: &Preferences, flavor_dir: &Path) -> Option<String> {
    let value = match &prefs.wow_installations {
        serde_json::Value::String(s) => serde_json::from_str(s).ok()?,
        v => v.clone(),
    };
    let installs: Vec<Installation> = serde_json::from_value(value).ok()?;
    let forever: Vec<&Installation> =
        installs.iter().filter(|i| i.client_type == FOREVER_CLIENT_TYPE).collect();
    forever
        .iter()
        .find(|i| Path::new(&i.location).starts_with(flavor_dir))
        .or(forever.first())
        .map(|i| i.id.clone())
}

fn list_folders(addons_dir: &Path) -> Result<Vec<String>, WowUpError> {
    let entries = match fs::read_dir(addons_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(WowUpError::Io { path: addons_dir.into(), source }),
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    Ok(names)
}

/// Keeps non-ignored records of `installation_id` whose folders exist on
/// disk, de-duplicated by `(provider, external_id)`: prefer the record with
/// all folders present, then the newest `installedAt`.
fn select_managed(
    records: impl Iterator<Item = AddonRecord>,
    installation_id: &str,
    on_disk: &[String],
) -> (Vec<ManagedAddon>, usize) {
    let present: BTreeSet<&str> = on_disk.iter().map(String::as_str).collect();
    let mut best: HashMap<(String, String), (bool, String, ManagedAddon)> = HashMap::new();
    let mut dropped = 0;

    for rec in records {
        if rec.installation_id != installation_id || rec.is_ignored {
            continue;
        }
        let Some(external_id) = rec.external_id.clone().filter(|s| !s.is_empty()) else {
            continue;
        };
        let folders = rec.folders();
        let found = folders.iter().filter(|f| present.contains(f.as_str())).count();
        if found == 0 {
            dropped += 1; // uninstalled outside WowUp, or a stale record
            continue;
        }
        let all_present = found == folders.len();
        let installed_at = rec.installed_at.clone().unwrap_or_default();
        let addon = ManagedAddon {
            name: rec.name,
            provider: rec.provider_name.clone(),
            external_id: external_id.clone(),
            version_id: rec.installed_external_release_id.filter(|s| !s.is_empty()),
            version: rec.installed_version.filter(|s| !s.is_empty()),
            folders,
        };
        let key = (rec.provider_name, external_id);
        match best.get(&key) {
            Some((a, t, _)) if (*a, t.as_str()) >= (all_present, installed_at.as_str()) => dropped += 1,
            Some(_) => {
                dropped += 1;
                best.insert(key, (all_present, installed_at, addon));
            }
            None => {
                best.insert(key, (all_present, installed_at, addon));
            }
        }
    }

    let mut managed: Vec<ManagedAddon> = best.into_values().map(|(_, _, a)| a).collect();
    managed.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.external_id.cmp(&b.external_id)));
    (managed, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(path: &Path, value: serde_json::Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn record(id: &str, name: &str, ext: &str, ver: &str, folders: &str, at: &str) -> serde_json::Value {
        json!({
            "id": id, "name": name, "installationId": "inst-forever", "providerName": "Curse",
            "externalId": ext, "installedExternalReleaseId": ver, "installedVersion": format!("v{ver}"),
            "installedFolders": folders, "installedAt": at, "isIgnored": false, "clientType": 10
        })
    }

    #[test]
    fn builds_inventory_with_dedupe_and_unmanaged() {
        let tmp = tempfile::tempdir().unwrap();
        let wowup = tmp.path().join("WowUpCf");
        let flavor = tmp.path().join("World of Warcraft/_forever_");
        let addons = flavor.join("Interface/AddOns");
        for f in ["PackA", "PackA_Options", "Single", "HandMade", "IgnoredOne"] {
            fs::create_dir_all(addons.join(f)).unwrap();
        }

        write(
            &wowup.join("preferences.json"),
            json!({ "wow_installations": [
                { "id": "inst-retail", "clientType": 0, "location": "/x/_retail_/World of Warcraft.app" },
                { "id": "inst-forever", "clientType": 10, "location": flavor.join("World of Warcraft.app") }
            ]}),
        );
        let mut ignored = record("5", "Ignored", "55", "1", "IgnoredOne", "2026-11-01T00:00:00Z");
        ignored["isIgnored"] = json!(true);
        let mut other_install = record("6", "Retail Only", "66", "1", "Single", "2026-11-01T00:00:00Z");
        other_install["installationId"] = json!("inst-retail");
        write(
            &wowup.join("addons.json"),
            json!({
                "1": record("1", "Pack A", "100", "10", "PackA,PackA_Options", "2026-11-05T00:00:00Z"),
                "2": record("2", "Pack A", "100", "9", "PackA,PackA_Options", "2026-11-01T00:00:00Z"),
                "3": record("3", "single", "200", "20", "Single", "2026-11-02T00:00:00Z"),
                "4": record("4", "Gone", "300", "30", "NotOnDisk", "2026-11-02T00:00:00Z"),
                "5": ignored,
                "6": other_install,
            }),
        );

        let inv = load_inventory(&[tmp.path().join("missing"), wowup.clone()], &flavor, &addons).unwrap();
        assert_eq!(inv.installation_id, "inst-forever");
        assert_eq!(inv.wowup_dir, wowup);
        let names: Vec<_> = inv.managed.iter().map(|a| (a.name.as_str(), a.version_id.as_deref())).collect();
        assert_eq!(names, [("Pack A", Some("10")), ("single", Some("20"))]);
        assert_eq!(inv.managed[0].folders, ["PackA", "PackA_Options"]);
        assert_eq!(inv.unmanaged_folders, ["HandMade", "IgnoredOne"]);
        assert_eq!(inv.dropped_records, 2); // older Pack A duplicate + Gone
    }

    #[test]
    fn errors_without_forever_installation() {
        let tmp = tempfile::tempdir().unwrap();
        let wowup = tmp.path().join("WowUp");
        write(&wowup.join("preferences.json"), json!({ "wow_installations": "[{\"id\":\"r\",\"clientType\":0}]" }));
        write(&wowup.join("addons.json"), json!({}));
        let err = load_inventory(&[wowup], tmp.path(), tmp.path()).unwrap_err();
        assert!(matches!(err, WowUpError::NoForeverInstallation));
    }

    #[test]
    fn errors_when_no_wowup_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_inventory(&[tmp.path().join("nope")], tmp.path(), tmp.path()).unwrap_err();
        assert!(matches!(err, WowUpError::NotFound(_)));
    }
}
