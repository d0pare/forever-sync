//! Writing repo state back into `_forever_/WTF` (DESIGN.md §7.2 step 6, §7.5).
//!
//! Safety rules:
//! * only paths under `wtf/` that pass [`rules::is_synced`] are ever written,
//!   whatever the repo contains;
//! * every overwritten or deleted file is copied to a backup folder first;
//! * writes are atomic (temp file + fsync + rename);
//! * a file is deleted only if it was in the last synced state (`base`), is
//!   gone from the target, and is still byte-identical on disk to `base`.
//!
//! Callers must check [`crate::process::forever_running`] first.
//! Backup folders older than [`BACKUP_KEEP`] are removed by [`purge_backups`].

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::layout::WTF_PREFIX;
use crate::{FileSet, rules};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyPlan {
    /// Repo paths to create or overwrite.
    pub writes: Vec<String>,
    /// Repo paths to delete.
    pub deletes: Vec<String>,
    /// Repo paths in the target that the rules don't allow (ignored).
    pub refused: Vec<String>,
}

impl ApplyPlan {
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.deletes.is_empty()
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ApplyReport {
    pub written: usize,
    pub deleted: usize,
    /// Where overwritten/deleted files were copied, if any were.
    pub backup_dir: Option<PathBuf>,
}

/// Maps a repo path (`wtf/Account/...`) to its location under `wtf_dir`, or
/// `None` if it is outside `wtf/`, malformed, or not allowed by the rules.
pub fn disk_path(wtf_dir: &Path, repo_path: &str) -> Option<PathBuf> {
    let rel = repo_path.strip_prefix(WTF_PREFIX)?;
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains('\\') || p.contains(':')) {
        return None;
    }
    if !rules::is_synced(&parts) {
        return None;
    }
    let mut out = wtf_dir.to_path_buf();
    for p in &parts {
        out.push(p);
    }
    // Belt and braces: the joined path must stay inside wtf_dir.
    let inside = Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)));
    inside.then_some(out)
}

/// Works out what applying `target` would change on disk. `base` is the last
/// state disk and repo agreed on (used only to decide deletions).
pub fn plan(wtf_dir: &Path, target: &FileSet, base: Option<&FileSet>) -> io::Result<ApplyPlan> {
    let mut plan = ApplyPlan::default();
    for (repo_path, bytes) in target.iter().filter(|(p, _)| p.starts_with(WTF_PREFIX)) {
        let Some(path) = disk_path(wtf_dir, repo_path) else {
            plan.refused.push(repo_path.clone());
            continue;
        };
        match fs::read(&path) {
            Ok(current) if current == *bytes => {}
            Ok(_) => plan.writes.push(repo_path.clone()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => plan.writes.push(repo_path.clone()),
            Err(e) => return Err(e),
        }
    }
    if let Some(base) = base {
        for (repo_path, base_bytes) in base.iter().filter(|(p, _)| p.starts_with(WTF_PREFIX)) {
            if target.contains_key(repo_path) {
                continue;
            }
            let Some(path) = disk_path(wtf_dir, repo_path) else { continue };
            match fs::read(&path) {
                Ok(current) if current == *base_bytes => plan.deletes.push(repo_path.clone()),
                Ok(_) => {} // changed locally since base: keep, next snapshot records it
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(plan)
}

/// Executes `plan`, backing up into a new folder under `backup_root`.
pub fn execute(wtf_dir: &Path, target: &FileSet, plan: &ApplyPlan, backup_root: &Path) -> io::Result<ApplyReport> {
    let mut report = ApplyReport::default();
    let backup_dir = backup_root.join(format!("{BACKUP_PREFIX}{}", now_ms()));

    let mut backup = |repo_path: &str, path: &Path| -> io::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        let rel = repo_path.strip_prefix(WTF_PREFIX).unwrap_or(repo_path);
        let dest = backup_dir.join(rel);
        fs::create_dir_all(dest.parent().expect("backup path has a parent"))?;
        fs::copy(path, &dest)?;
        report.backup_dir = Some(backup_dir.clone());
        Ok(())
    };

    for repo_path in &plan.writes {
        let path = disk_path(wtf_dir, repo_path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("refusing to write {repo_path}")))?;
        let bytes = target
            .get(repo_path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{repo_path} not in target")))?;
        backup(repo_path, &path)?;
        write_atomic(&path, bytes)?;
        report.written += 1;
    }
    for repo_path in &plan.deletes {
        let Some(path) = disk_path(wtf_dir, repo_path) else { continue };
        backup(repo_path, &path)?;
        match fs::remove_file(&path) {
            Ok(()) => report.deleted += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(report)
}

/// How long [`execute`]'s backups are kept (DESIGN.md §7.5). Every version
/// is also in the repo's history; backups cover the moment of the write.
pub const BACKUP_KEEP: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const BACKUP_PREFIX: &str = "apply-";

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Deletes `apply-<ms>` backup folders older than `keep`; anything else in
/// `backup_root` is left alone. Returns how many were removed.
pub fn purge_backups(backup_root: &Path, keep: Duration) -> io::Result<usize> {
    let entries = match fs::read_dir(backup_root) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let now = now_ms();
    let mut removed = 0;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(stamp) = name.to_str().and_then(|n| n.strip_prefix(BACKUP_PREFIX)).and_then(|n| n.parse::<u128>().ok())
        else {
            continue;
        };
        if now.saturating_sub(stamp) > keep.as_millis() && entry.file_type().is_ok_and(|t| t.is_dir()) {
            fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().expect("file path has a parent");
    fs::create_dir_all(dir)?;
    let mut tmp_name = path.file_name().expect("file path has a name").to_os_string();
    tmp_name.push(".foreversync-tmp");
    let tmp = dir.join(tmp_name);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, &str)]) -> FileSet {
        pairs.iter().map(|(p, c)| (p.to_string(), c.as_bytes().to_vec())).collect()
    }

    #[test]
    fn disk_path_enforces_rules() {
        let w = Path::new("/w");
        assert_eq!(
            disk_path(w, "wtf/Account/A/SavedVariables/X.lua"),
            Some(PathBuf::from("/w/Account/A/SavedVariables/X.lua"))
        );
        assert_eq!(disk_path(w, "wtf/Config.wtf"), None);
        assert_eq!(disk_path(w, "wtf/Account/../Config.wtf"), None);
        assert_eq!(disk_path(w, "foreversync.json"), None);
        assert_eq!(disk_path(w, "wtf/Account/A/SavedVariables/X.lua.bak"), None);
    }

    #[test]
    fn plans_and_executes_with_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let wtf = tmp.path().join("WTF");
        let backups = tmp.path().join("backups");
        let p = |rel: &str| wtf.join(rel);
        fs::create_dir_all(p("Account/A/SavedVariables")).unwrap();
        fs::write(p("Account/A/SavedVariables/Same.lua"), "S = 1\n").unwrap();
        fs::write(p("Account/A/SavedVariables/Changed.lua"), "C = 1\n").unwrap();
        fs::write(p("Account/A/SavedVariables/Gone.lua"), "G = 1\n").unwrap();
        fs::write(p("Account/A/SavedVariables/LocalEdit.lua"), "L = 2\n").unwrap();
        fs::write(p("Config.wtf"), "SET x 1\n").unwrap();

        let base = set(&[
            ("wtf/Account/A/SavedVariables/Same.lua", "S = 1\n"),
            ("wtf/Account/A/SavedVariables/Changed.lua", "C = 1\n"),
            ("wtf/Account/A/SavedVariables/Gone.lua", "G = 1\n"),
            ("wtf/Account/A/SavedVariables/LocalEdit.lua", "L = 1\n"),
        ]);
        let target = set(&[
            ("foreversync.json", "{}"),
            ("wtf/Account/A/SavedVariables/Same.lua", "S = 1\n"),
            ("wtf/Account/A/SavedVariables/Changed.lua", "C = 2\n"),
            ("wtf/Account/A/93/New-Char/AddOns.txt", "Plater: enabled\n"),
            ("wtf/Config.wtf", "SET hacked 1\n"),
        ]);

        let plan = plan(&wtf, &target, Some(&base)).unwrap();
        assert_eq!(
            plan.writes,
            ["wtf/Account/A/93/New-Char/AddOns.txt", "wtf/Account/A/SavedVariables/Changed.lua"]
        );
        assert_eq!(plan.deletes, ["wtf/Account/A/SavedVariables/Gone.lua"]);
        assert_eq!(plan.refused, ["wtf/Config.wtf"]);

        let report = execute(&wtf, &target, &plan, &backups).unwrap();
        assert_eq!((report.written, report.deleted), (2, 1));
        assert_eq!(fs::read_to_string(p("Account/A/SavedVariables/Changed.lua")).unwrap(), "C = 2\n");
        assert_eq!(fs::read_to_string(p("Account/A/93/New-Char/AddOns.txt")).unwrap(), "Plater: enabled\n");
        assert!(!p("Account/A/SavedVariables/Gone.lua").exists());
        assert_eq!(fs::read_to_string(p("Account/A/SavedVariables/LocalEdit.lua")).unwrap(), "L = 2\n");
        assert_eq!(fs::read_to_string(p("Config.wtf")).unwrap(), "SET x 1\n");

        let bdir = report.backup_dir.unwrap();
        assert_eq!(fs::read_to_string(bdir.join("Account/A/SavedVariables/Changed.lua")).unwrap(), "C = 1\n");
        assert_eq!(fs::read_to_string(bdir.join("Account/A/SavedVariables/Gone.lua")).unwrap(), "G = 1\n");
        assert!(!bdir.join("Account/A/93/New-Char/AddOns.txt").exists(), "new files need no backup");

        // Re-planning after execute is a no-op.
        assert!(super::plan(&wtf, &target, Some(&base)).unwrap().writes.is_empty());
        let leftovers: Vec<_> = fs::read_dir(p("Account/A/SavedVariables"))
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".foreversync-tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn purges_only_old_apply_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let old = now_ms() - BACKUP_KEEP.as_millis() - 1000;
        let recent = now_ms() - 1000;
        for name in [format!("apply-{old}"), format!("apply-{recent}"), "apply-x".into(), "keep-me".into()] {
            fs::create_dir_all(root.join(&name).join("Account")).unwrap();
            fs::write(root.join(&name).join("Account/f.lua"), "x").unwrap();
        }
        fs::write(root.join(format!("apply-{}", old - 1)), "a file, not a backup").unwrap();

        assert_eq!(purge_backups(root, BACKUP_KEEP).unwrap(), 1);
        assert!(!root.join(format!("apply-{old}")).exists());
        for name in [format!("apply-{recent}"), "apply-x".into(), "keep-me".into(), format!("apply-{}", old - 1)] {
            assert!(root.join(&name).exists(), "{name} kept");
        }
        assert_eq!(purge_backups(&root.join("missing"), BACKUP_KEEP).unwrap(), 0);
    }
}
