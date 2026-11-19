//! Reads the syncable part of `_forever_/WTF` into a [`FileSet`]. Read-only.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::layout::WTF_PREFIX;
use crate::{FileSet, lua, rules};

#[derive(Debug, Default, Serialize)]
pub struct ScanResult {
    /// Repo path (`wtf/...`) → bytes, for every file that passed the rules.
    pub files: FileSet,
    /// Files the rules allow but that were left out, with the reason.
    pub skipped: Vec<Skipped>,
    /// Synced files over [`rules::WARN_FILE_BYTES`].
    pub large: Vec<(String, u64)>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

/// Scans `wtf_dir` (the `_forever_/WTF` folder).
pub fn scan_wtf(wtf_dir: &Path) -> io::Result<ScanResult> {
    let mut out = ScanResult::default();
    let mut stack: Vec<(std::path::PathBuf, Vec<String>)> = vec![(wtf_dir.to_path_buf(), Vec::new())];

    while let Some((dir, rel)) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound && rel.is_empty() => return Ok(out),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else {
                out.skipped.push(Skipped {
                    path: entry.path().display().to_string(),
                    reason: "file name is not valid Unicode".into(),
                });
                continue;
            };
            let name: String = name.nfc().collect();
            let mut child = rel.clone();
            child.push(name);

            if ft.is_dir() {
                stack.push((entry.path(), child));
                continue;
            }
            let parts: Vec<&str> = child.iter().map(String::as_str).collect();
            if !rules::is_synced(&parts) {
                continue;
            }
            let repo_path = format!("{WTF_PREFIX}{}", child.join("/"));
            let size = entry.metadata()?.len();
            if size > rules::MAX_FILE_BYTES {
                out.skipped.push(Skipped {
                    path: repo_path,
                    reason: format!("too large ({} MB)", size / (1024 * 1024)),
                });
                continue;
            }
            let bytes = fs::read(entry.path())?;
            if repo_path.to_ascii_lowercase().ends_with(".lua") {
                if let Err(e) = lua::check(&bytes) {
                    out.skipped.push(Skipped { path: repo_path, reason: format!("invalid Lua: {e}") });
                    continue;
                }
            }
            if size > rules::WARN_FILE_BYTES {
                out.large.push((repo_path.clone(), size));
            }
            out.files.insert(repo_path, bytes);
        }
    }
    out.skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn scans_with_rules_and_lua_check() {
        let tmp = tempfile::tempdir().unwrap();
        let wtf = tmp.path();
        put(wtf, "Config.wtf", "SET accountName \"secret@example.com\"\n");
        put(wtf, "Account/A/SavedVariables/Good.lua", "Good = {\n}\n");
        put(wtf, "Account/A/SavedVariables/Good.lua.bak", "old");
        put(wtf, "Account/A/SavedVariables/Broken.lua", "Broken = {\n[\"x\"] = ");
        put(wtf, "Account/A/bindings-cache.wtf", "BINDINGMODE 0\n");
        put(wtf, "Account/A/93/Char-One/AddOns.txt", "Plater: enabled\n");

        let r = scan_wtf(wtf).unwrap();
        let paths: Vec<_> = r.files.keys().map(String::as_str).collect();
        assert_eq!(
            paths,
            [
                "wtf/Account/A/93/Char-One/AddOns.txt",
                "wtf/Account/A/SavedVariables/Good.lua",
                "wtf/Account/A/bindings-cache.wtf",
            ]
        );
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].path, "wtf/Account/A/SavedVariables/Broken.lua");
        assert!(r.skipped[0].reason.starts_with("invalid Lua"));
    }

    #[test]
    fn missing_wtf_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let r = scan_wtf(&tmp.path().join("nope")).unwrap();
        assert!(r.files.is_empty());
    }
}
