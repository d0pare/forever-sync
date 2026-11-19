//! Every WoW Forever–specific fact lives here (DESIGN.md §3.6), so a Blizzard
//! layout change is a one-file fix. Values were verified against a real
//! install of client 1.60.1.70938 on 2026-11-18.

/// Subfolder of the WoW install root that holds the Forever client.
pub const FLAVOR_DIR: &str = "_forever_";

/// Battle.net product code, found in `.build.info` and `.flavor.info`.
pub const PRODUCT_CODE: &str = "wowf";

/// File inside [`FLAVOR_DIR`] whose second line is the product code.
pub const FLAVOR_INFO_FILE: &str = ".flavor.info";

/// Pipe-separated build table in the install root.
pub const BUILD_INFO_FILE: &str = ".build.info";

/// Folder names under `WTF/Account` (and under each account) that are not
/// accounts/realms but Blizzard SavedVariables directories.
pub const SAVED_VARIABLES_DIR: &str = "SavedVariables";

/// TOC `## Interface` value for patch 1.60.1.
pub const INTERFACE_VERSIONS: &[u32] = &[16001];

/// Flavor-specific TOC suffixes used by Forever addons ("Camelot" was the
/// internal codename).
pub const TOC_SUFFIXES: &[&str] = &["_Forever", "_Camelot"];

/// WowUp's `WowClientType` enum value for Forever.
pub const WOWUP_CLIENT_TYPE: i64 = 10;

/// WowUp's `client_type` string in import/export payloads.
pub const WOWUP_CLIENT_TYPE_NAME: &str = "Forever";

/// Install roots checked when Battle.net's product.db can't be read.
pub fn default_install_roots() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &["/Applications/World of Warcraft"]
    }
    #[cfg(target_os = "windows")]
    {
        &[
            r"C:\Program Files (x86)\World of Warcraft",
            r"C:\Program Files\World of Warcraft",
        ]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        &[]
    }
}
