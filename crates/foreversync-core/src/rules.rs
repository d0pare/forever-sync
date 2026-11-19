//! What gets synced (DESIGN.md §3.5). Paths are given as components relative
//! to `_forever_/WTF`. Everything not explicitly allowed is skipped, so new
//! Blizzard files are never synced by accident.

use crate::forever::SAVED_VARIABLES_DIR;

/// Warn when a synced file is larger than this.
pub const WARN_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// Never sync files larger than this (GitHub rejects files over 100 MB).
pub const MAX_FILE_BYTES: u64 = 95 * 1024 * 1024;

/// Should the file at `rel` (relative to `WTF/`) be synced?
///
/// Synced:
/// * `Account/SavedVariables/*.lua` (account-agnostic Blizzard settings)
/// * `Account/<ACCT>/SavedVariables/*.lua`
/// * `Account/<ACCT>/*.wtf|*.txt` (bindings, macros, edit mode, chat, …)
/// * `Account/<ACCT>/<REALM>/<CHAR>/*.wtf|*.txt` (AddOns.txt, layout, …)
/// * `Account/<ACCT>/<REALM>/<CHAR>/SavedVariables/*.lua`
///
/// Never: `Config.wtf`, global `WTF/SavedVariables`, gamepad files, `*.bak`,
/// `*.old`, `cache.md5`, dotfiles, anything else.
pub fn is_synced(rel: &[&str]) -> bool {
    let Some((file, dirs)) = rel.split_last() else { return false };
    if dirs.first() != Some(&"Account") || dirs.iter().any(|d| d.starts_with('.')) {
        return false;
    }
    let lower = file.to_ascii_lowercase();
    if lower.starts_with('.') || lower == "cache.md5" {
        return false;
    }
    let is_lua = lower.ends_with(".lua");
    let is_cfg = lower.ends_with(".wtf") || lower.ends_with(".txt");
    let sv = SAVED_VARIABLES_DIR;

    match dirs {
        ["Account", d] if *d == sv => is_lua,
        ["Account", acct] if *acct != sv => is_cfg,
        ["Account", acct, d] if *acct != sv && *d == sv => is_lua,
        ["Account", acct, realm, _char] if *acct != sv && *realm != sv => is_cfg,
        ["Account", acct, realm, _char, d] if *acct != sv && *realm != sv && *d == sv => is_lua,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synced(path: &str) -> bool {
        is_synced(&path.split('/').collect::<Vec<_>>())
    }

    #[test]
    fn includes_settings() {
        for p in [
            "Account/SavedVariables/Blizzard_SavedSets.lua",
            "Account/ACCT/SavedVariables/Plater.lua",
            "Account/ACCT/bindings-cache.wtf",
            "Account/ACCT/config-cache.wtf",
            "Account/ACCT/macros-cache.txt",
            "Account/ACCT/edit-mode-cache-account.txt",
            "Account/ACCT/character-list-order.txt",
            "Account/ACCT/93/Char-Name/AddOns.txt",
            "Account/ACCT/93/Char-Name/layout-local.txt",
            "Account/ACCT/93/Char-Name/SavedVariables/PlaterDBChr.lua",
        ] {
            assert!(synced(p), "{p} should sync");
        }
    }

    #[test]
    fn excludes_everything_else() {
        for p in [
            "Config.wtf",
            "GamePadConfig_Default.json",
            "gamecontrollerdb.txt",
            "SavedVariables/Blizzard_Console.lua",
            "Account/ACCT/SavedVariables/Plater.lua.bak",
            "Account/ACCT/macros-cache.old",
            "Account/ACCT/cache.md5",
            "Account/ACCT/.DS_Store",
            "Account/ACCT/SavedVariables/notes.txt",
            "Account/ACCT/93/realm-level.txt",
            "Account/ACCT/93/Char/SavedVariables/x.lua.bak",
            "Account/ACCT/93/Char/Extra/deep.lua",
            "Account/SavedVariables/x.txt",
            "Account/top.txt",
            "Other/ACCT/SavedVariables/x.lua",
        ] {
            assert!(!synced(p), "{p} should not sync");
        }
    }
}
