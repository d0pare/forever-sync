//! Git storage for Forever Sync (DESIGN.md §5).
//!
//! The repo is **bare** and lives in app data. Trees are built straight from
//! in-memory file sets (repo path → bytes), so the game folder is never a git
//! working tree. Remote fetch/push and 3-way merge arrive in M2.

use std::collections::BTreeMap;
use std::path::Path;

use git2::{ObjectType, Oid, Repository, Signature, TreeWalkMode, TreeWalkResult};

/// Repo-relative path (`/`-separated, NFC) → file bytes.
pub type FileSet = BTreeMap<String, Vec<u8>>;

pub const BRANCH_REF: &str = "refs/heads/main";

const FILE_MODE: i32 = 0o100644;
const DIR_MODE: i32 = 0o040000;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error("invalid repo path `{0}`")]
    BadPath(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: String,
    pub time: i64,
    pub summary: String,
    pub message: String,
}

pub struct Store {
    repo: Repository,
}

impl Store {
    /// Opens the bare repo at `path`, creating it (with `main` as HEAD) if needed.
    pub fn open_or_init(path: &Path) -> Result<Self> {
        let repo = match Repository::open_bare(path) {
            Ok(r) => r,
            Err(_) => {
                let mut opts = git2::RepositoryInitOptions::new();
                opts.bare(true).initial_head("main").mkpath(true);
                Repository::init_opts(path, &opts)?
            }
        };
        Ok(Self { repo })
    }

    /// The commit `main` points at, if any.
    pub fn head(&self) -> Result<Option<Oid>> {
        match self.repo.refname_to_id(BRANCH_REF) {
            Ok(id) => Ok(Some(id)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Commits `files` as the new tree of `main`. Returns `None` (and makes no
    /// commit) when the tree equals the current head's tree.
    pub fn snapshot(&self, files: &FileSet, message: &str) -> Result<Option<Oid>> {
        let tree_id = self.write_tree(files)?;
        let parent = match self.head()? {
            Some(id) => Some(self.repo.find_commit(id)?),
            None => None,
        };
        if parent.as_ref().is_some_and(|p| p.tree_id() == tree_id) {
            return Ok(None);
        }
        let tree = self.repo.find_tree(tree_id)?;
        let sig = Signature::now("Forever Sync", "forever-sync@localhost")?;
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        let id = self.repo.commit(Some(BRANCH_REF), &sig, &sig, message, &tree, &parents)?;
        Ok(Some(id))
    }

    /// Full commit id for `rev` (a commit id prefix, ref, `main~1`, …).
    pub fn resolve(&self, rev: &str) -> Result<String> {
        Ok(self.repo.revparse_single(rev)?.peel_to_commit()?.id().to_string())
    }

    /// All files in the tree of `rev` (a commit id, ref or `main`).
    pub fn files_at(&self, rev: &str) -> Result<FileSet> {
        let commit = self.repo.revparse_single(rev)?.peel_to_commit()?;
        let tree = commit.tree()?;
        let mut out = FileSet::new();
        let mut err = None;
        tree.walk(TreeWalkMode::PreOrder, |root, entry| {
            if entry.kind() != Some(ObjectType::Blob) {
                return TreeWalkResult::Ok;
            }
            let name = match entry.name() {
                Ok(n) => n,
                Err(_) => {
                    err = Some(StoreError::BadPath(format!("{root}<non-utf8>")));
                    return TreeWalkResult::Abort;
                }
            };
            match self.repo.find_blob(entry.id()) {
                Ok(blob) => {
                    out.insert(format!("{root}{name}"), blob.content().to_vec());
                    TreeWalkResult::Ok
                }
                Err(e) => {
                    err = Some(e.into());
                    TreeWalkResult::Abort
                }
            }
        })?;
        match err {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    /// Files at the head of `main`, or an empty set for a fresh repo.
    pub fn head_files(&self) -> Result<FileSet> {
        match self.head()? {
            Some(id) => self.files_at(&id.to_string()),
            None => Ok(FileSet::new()),
        }
    }

    /// The newest `limit` commits on `main`, newest first.
    pub fn log(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let Some(head) = self.head()? else { return Ok(Vec::new()) };
        let mut walk = self.repo.revwalk()?;
        walk.push(head)?;
        walk.take(limit)
            .map(|id| {
                let c = self.repo.find_commit(id?)?;
                Ok(CommitInfo {
                    id: c.id().to_string(),
                    time: c.time().seconds(),
                    summary: c.summary().ok().flatten().unwrap_or("").to_owned(),
                    message: c.message().unwrap_or("").to_owned(),
                })
            })
            .collect()
    }

    fn write_tree(&self, files: &FileSet) -> Result<Oid> {
        #[derive(Default)]
        struct Dir {
            files: BTreeMap<String, Oid>,
            dirs: BTreeMap<String, Dir>,
        }
        let mut root = Dir::default();
        for (path, bytes) in files {
            let parts: Vec<&str> = path.split('/').collect();
            if parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
                return Err(StoreError::BadPath(path.clone()));
            }
            let (file, dirs) = parts.split_last().expect("split always yields one part");
            let mut node = &mut root;
            for d in dirs {
                node = node.dirs.entry((*d).to_owned()).or_default();
            }
            node.files.insert((*file).to_owned(), self.repo.blob(bytes)?);
        }
        fn write(repo: &Repository, dir: &Dir) -> Result<Oid> {
            let mut tb = repo.treebuilder(None)?;
            for (name, id) in &dir.files {
                tb.insert(name, *id, FILE_MODE)?;
            }
            for (name, sub) in &dir.dirs {
                if dir.files.contains_key(name) {
                    return Err(StoreError::BadPath(name.clone()));
                }
                tb.insert(name, write(repo, sub)?, DIR_MODE)?;
            }
            Ok(tb.write()?)
        }
        write(&self.repo, &root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, &str)]) -> FileSet {
        pairs.iter().map(|(p, c)| (p.to_string(), c.as_bytes().to_vec())).collect()
    }

    #[test]
    fn snapshot_round_trip_and_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open_or_init(&tmp.path().join("r.git")).unwrap();
        assert_eq!(store.head().unwrap(), None);
        assert!(store.head_files().unwrap().is_empty());

        let files = set(&[
            (".gitattributes", "* -text\n"),
            ("wtf/Account/A/SavedVariables/X.lua", "X = {\r\n}\r\n"),
            ("wtf/Account/A/93/Char-One/AddOns.txt", "Foo: enabled\n"),
            ("wtf/Account/A/93/Char-Ünï/AddOns.txt", "Bar: enabled\n"),
        ]);
        let first = store.snapshot(&files, "first").unwrap().expect("commit");
        assert_eq!(store.files_at(&first.to_string()).unwrap(), files);
        assert_eq!(store.snapshot(&files, "again").unwrap(), None, "unchanged tree → no commit");

        let mut changed = files.clone();
        changed.remove("wtf/Account/A/93/Char-One/AddOns.txt");
        changed.insert("wtf/Account/A/SavedVariables/X.lua".into(), b"X = {1}\n".to_vec());
        let second = store.snapshot(&changed, "second").unwrap().expect("commit");
        assert_eq!(store.head_files().unwrap(), changed);
        assert_eq!(store.files_at(&first.to_string()).unwrap(), files, "history kept");

        let log = store.log(10).unwrap();
        assert_eq!(log.iter().map(|c| c.summary.as_str()).collect::<Vec<_>>(), ["second", "first"]);
        assert_eq!(log[0].id, second.to_string());
    }

    #[test]
    fn rejects_bad_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open_or_init(&tmp.path().join("r.git")).unwrap();
        for bad in ["a//b", "../x", "a/./b", "/abs"] {
            assert!(matches!(store.snapshot(&set(&[(bad, "x")]), "m"), Err(StoreError::BadPath(_))), "{bad}");
        }
    }
}
