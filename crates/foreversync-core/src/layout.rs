//! Data repo layout (DESIGN.md §5.2).

use crate::FileSet;

/// Prefix of the synced `WTF/` mirror inside the repo.
pub const WTF_PREFIX: &str = "wtf/";

/// Repo schema version written to `foreversync.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// Files every data repo carries at its root.
pub fn base_files() -> FileSet {
    let mut f = FileSet::new();
    f.insert(".gitattributes".into(), b"* -text\n".to_vec());
    f.insert(
        "foreversync.json".into(),
        format!("{{\n  \"schema\": {SCHEMA_VERSION}\n}}\n").into_bytes(),
    );
    f.insert(
        "README.md".into(),
        b"# Forever Sync data\n\n\
This repository is written by the Forever Sync app. It holds World of Warcraft: Forever \
settings (SavedVariables, keybindings, macros) and the WowUp addon list.\n\n\
**Keep this repository private.** SavedVariables can contain chat history and character data.\n"
            .to_vec(),
    );
    f
}

/// Only the `wtf/` part of a repo file set.
pub fn wtf_only(files: &FileSet) -> FileSet {
    files
        .iter()
        .filter(|(p, _)| p.starts_with(WTF_PREFIX))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}
