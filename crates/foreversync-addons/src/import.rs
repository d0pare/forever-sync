//! WowUp import/export payloads (DESIGN.md §6.3).
//!
//! Format taken from WowUp-CF 2.25.5 (`getExportPayload` / `parseImportString`):
//! base64 of `{"collection_name", "client_type", "addons": [{id, name,
//! provider_name, version_id}]}`. WowUp also accepts the raw JSON.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::wowup::ManagedAddon;

/// WowUp's `client_type` name for Forever.
pub const CLIENT_TYPE_NAME: &str = "Forever";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportPayload {
    pub collection_name: String,
    pub client_type: String,
    pub addons: Vec<ImportAddon>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportAddon {
    pub id: String,
    pub name: String,
    pub provider_name: String,
    /// WowUp omits this key when unknown (`?? undefined`), so we do too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
}

impl ImportPayload {
    pub fn from_addons(addons: &[ManagedAddon], created_unix_ms: u128) -> Self {
        Self {
            collection_name: format!("ForeverSync_{created_unix_ms}"),
            client_type: CLIENT_TYPE_NAME.to_owned(),
            addons: addons
                .iter()
                .map(|a| ImportAddon {
                    id: a.external_id.clone(),
                    name: a.name.clone(),
                    provider_name: a.provider.clone(),
                    version_id: a.version_id.clone(),
                })
                .collect(),
        }
    }
}

/// The string to paste into WowUp → My Addons → Import/Export Addons.
pub fn import_string(payload: &ImportPayload) -> String {
    let json = serde_json::to_string(payload).expect("payload is always serializable");
    base64::engine::general_purpose::STANDARD.encode(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_base64_in_wowup_shape() {
        let addons = vec![
            ManagedAddon {
                name: "Enhance QoL".into(),
                provider: "Curse".into(),
                external_id: "1076354".into(),
                version_id: Some("7123456".into()),
                version: Some("13.23.0-forever".into()),
                folders: vec!["EnhanceQoL".into()],
            },
            ManagedAddon {
                name: "NoVersion".into(),
                provider: "WowUpHub".into(),
                external_id: "47671".into(),
                version_id: None,
                version: None,
                folders: vec!["NoVersion".into()],
            },
        ];
        let payload = ImportPayload::from_addons(&addons, 1_700_000_000_000);
        let s = import_string(&payload);

        let decoded = base64::engine::general_purpose::STANDARD.decode(&s).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(value["collection_name"], "ForeverSync_1700000000000");
        assert_eq!(value["client_type"], "Forever");
        assert_eq!(
            value["addons"][0],
            serde_json::json!({"id": "1076354", "name": "Enhance QoL", "provider_name": "Curse", "version_id": "7123456"})
        );
        assert!(value["addons"][1].get("version_id").is_none());
        let back: ImportPayload = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(back, payload);
    }
}
