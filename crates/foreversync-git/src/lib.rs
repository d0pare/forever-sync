//! Git storage for Forever Sync (DESIGN.md §5).
//!
//! The repo is **bare** and lives in app data. Trees are built straight from
//! in-memory file sets (repo path → bytes), so the game folder is never a git
//! working tree. Remote fetch/push and 3-way merge arrive in M2.

use std::collections::BTreeMap;
use std::path::Path;

use git2::{ObjectType, Repository, Signature, TreeWalkMode, TreeWalkResult};

pub use git2::Oid;

/// Repo-relative path (`/`-separated, NFC) → file bytes.
pub type FileSet = BTreeMap<String, Vec<u8>>;

pub const BRANCH_REF: &str = "refs/heads/main";
/// Where the last fetched remote `main` is kept.
pub const REMOTE_REF: &str = "refs/remotes/origin/main";

const FILE_MODE: i32 = 0o100644;
const DIR_MODE: i32 = 0o040000;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error("invalid repo path `{0}`")]
    BadPath(String),
    #[error("authentication failed for {0}")]
    Auth(String),
}

/// Where to fetch from / push to, and the token for HTTPS (`None` for
/// local `file://` remotes in tests).
#[derive(Clone)]
pub struct Remote {
    pub url: String,
    pub token: Option<String>,
}

impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remote").field("url", &self.url).field("token", &self.token.as_ref().map(|_| "***")).finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    Pushed,
    /// The remote moved on (non-fast-forward); fetch, merge, retry.
    Rejected(String),
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

    /// Commits `files` with explicit `parents` and moves `main` to it.
    pub fn commit_merge(&self, files: &FileSet, message: &str, parents: &[Oid]) -> Result<Oid> {
        let tree = self.repo.find_tree(self.write_tree(files)?)?;
        let parents: Vec<git2::Commit> = parents.iter().map(|p| self.repo.find_commit(*p)).collect::<std::result::Result<_, _>>()?;
        let refs: Vec<&git2::Commit> = parents.iter().collect();
        let sig = Signature::now("Forever Sync", "forever-sync@localhost")?;
        Ok(self.repo.commit(Some(BRANCH_REF), &sig, &sig, message, &tree, &refs)?)
    }

    /// Points `main` at `id` (fast-forward or adopting the remote history).
    pub fn set_head(&self, id: Oid) -> Result<()> {
        self.repo.reference(BRANCH_REF, id, true, "forever-sync: move main")?;
        Ok(())
    }

    /// Commit time (unix seconds) of `id`.
    pub fn commit_time(&self, id: Oid) -> Result<i64> {
        Ok(self.repo.find_commit(id)?.time().seconds())
    }

    /// Best common ancestor, or `None` for unrelated histories.
    pub fn merge_base(&self, a: Oid, b: Oid) -> Result<Option<Oid>> {
        match self.repo.merge_base(a, b) {
            Ok(id) => Ok(Some(id)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Is `ancestor` reachable from `descendant` (or equal to it)?
    pub fn is_ancestor(&self, ancestor: Oid, descendant: Oid) -> Result<bool> {
        Ok(ancestor == descendant || self.repo.graph_descendant_of(descendant, ancestor)?)
    }

    /// Files recorded under a local-only ref (e.g. last observed disk state).
    pub fn files_at_ref(&self, refname: &str) -> Result<Option<FileSet>> {
        match self.repo.refname_to_id(refname) {
            Ok(id) => self.files_at(&id.to_string()).map(Some),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Records `files` under a local-only ref as a parentless commit.
    pub fn set_files_ref(&self, refname: &str, files: &FileSet, message: &str) -> Result<()> {
        let tree = self.repo.find_tree(self.write_tree(files)?)?;
        let sig = Signature::now("Forever Sync", "forever-sync@localhost")?;
        let id = self.repo.commit(None, &sig, &sig, message, &tree, &[])?;
        self.repo.reference(refname, id, true, message)?;
        Ok(())
    }

    /// Last fetched remote `main`.
    pub fn remote_head(&self) -> Result<Option<Oid>> {
        match self.repo.refname_to_id(REMOTE_REF) {
            Ok(id) => Ok(Some(id)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Fetches the remote `main` into [`REMOTE_REF`]. Returns its id, or
    /// `None` if the remote has no `main` yet (empty repo).
    pub fn fetch(&self, remote: &Remote) -> Result<Option<Oid>> {
        let mut r = self.repo.remote_anonymous(&remote.url)?;
        let mut opts = git2::FetchOptions::new();
        opts.remote_callbacks(callbacks(remote, None));
        let refspec = format!("+{BRANCH_REF}:{REMOTE_REF}");
        match r.fetch(&[refspec.as_str()], Some(&mut opts), Some("forever-sync: fetch")) {
            Ok(()) => {}
            Err(e) if e.class() == git2::ErrorClass::Http && e.code() == git2::ErrorCode::Auth => {
                return Err(StoreError::Auth(remote.url.clone()));
            }
            Err(e) => return Err(e.into()),
        }
        // An empty remote advertises no `main`; clear any stale tracking ref.
        let advertised = r.list()?.iter().any(|h| h.name() == BRANCH_REF);
        if !advertised {
            if let Ok(mut stale) = self.repo.find_reference(REMOTE_REF) {
                stale.delete()?;
            }
            return Ok(None);
        }
        self.remote_head()
    }

    /// Pushes local `main` to the remote `main` (never forced).
    pub fn push(&self, remote: &Remote) -> Result<PushOutcome> {
        let Some(head) = self.head()? else { return Ok(PushOutcome::Pushed) };
        let mut r = self.repo.remote_anonymous(&remote.url)?;
        let rejected = std::cell::RefCell::new(None::<String>);
        let mut opts = git2::PushOptions::new();
        opts.remote_callbacks(callbacks(remote, Some(&rejected)));
        let refspec = format!("{BRANCH_REF}:{BRANCH_REF}");
        match r.push(&[refspec.as_str()], Some(&mut opts)) {
            Ok(()) => {}
            Err(e) if e.code() == git2::ErrorCode::NotFastForward => return Ok(PushOutcome::Rejected(e.message().to_owned())),
            Err(e) if e.code() == git2::ErrorCode::Auth => return Err(StoreError::Auth(remote.url.clone())),
            Err(e) => return Err(e.into()),
        }
        drop(opts);
        if let Some(msg) = rejected.into_inner() {
            return Ok(PushOutcome::Rejected(msg));
        }
        self.repo.reference(REMOTE_REF, head, true, "forever-sync: push")?;
        Ok(PushOutcome::Pushed)
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

fn callbacks<'a>(remote: &'a Remote, rejected: Option<&'a std::cell::RefCell<Option<String>>>) -> git2::RemoteCallbacks<'a> {
    let mut cb = git2::RemoteCallbacks::new();
    let mut attempts = 0;
    cb.credentials(move |_url, _user, _allowed| {
        attempts += 1;
        match (&remote.token, attempts) {
            (Some(token), 1) => git2::Cred::userpass_plaintext("x-access-token", token),
            _ => Err(git2::Error::new(git2::ErrorCode::Auth, git2::ErrorClass::Http, "credentials rejected")),
        }
    });
    if let Some(rejected) = rejected {
        cb.push_update_reference(move |_refname, status| {
            if let Some(msg) = status {
                *rejected.borrow_mut() = Some(msg.to_owned());
            }
            Ok(())
        });
    }
    cb
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

    fn bare_remote(dir: &Path) -> Remote {
        Repository::init_bare(dir).unwrap();
        Remote { url: format!("file://{}", dir.display()), token: None }
    }

    #[test]
    fn fetch_push_merge_base_and_rejection() {
        let tmp = tempfile::tempdir().unwrap();
        let remote = bare_remote(&tmp.path().join("origin.git"));
        let a = Store::open_or_init(&tmp.path().join("a.git")).unwrap();
        let b = Store::open_or_init(&tmp.path().join("b.git")).unwrap();

        assert_eq!(a.fetch(&remote).unwrap(), None, "empty remote");
        let c1 = a.snapshot(&set(&[("f", "1")]), "c1").unwrap().unwrap();
        assert_eq!(a.push(&remote).unwrap(), PushOutcome::Pushed);
        assert_eq!(a.remote_head().unwrap(), Some(c1));

        assert_eq!(b.fetch(&remote).unwrap(), Some(c1));
        b.set_head(c1).unwrap();
        let c2b = b.snapshot(&set(&[("f", "2b")]), "c2b").unwrap().unwrap();
        let c2a = a.snapshot(&set(&[("f", "2a")]), "c2a").unwrap().unwrap();
        assert_eq!(a.push(&remote).unwrap(), PushOutcome::Pushed);

        assert!(matches!(b.push(&remote).unwrap(), PushOutcome::Rejected(_)), "b is behind");
        let theirs = b.fetch(&remote).unwrap().unwrap();
        assert_eq!(theirs, c2a);
        assert_eq!(b.merge_base(c2b, c2a).unwrap(), Some(c1));
        assert!(b.is_ancestor(c1, c2b).unwrap());
        assert!(!b.is_ancestor(c2a, c2b).unwrap());
        let m = b.commit_merge(&set(&[("f", "merged")]), "merge", &[c2b, c2a]).unwrap();
        assert_eq!(b.push(&remote).unwrap(), PushOutcome::Pushed);
        assert_eq!(a.fetch(&remote).unwrap(), Some(m));
        assert!(a.is_ancestor(c2a, m).unwrap());
    }

    #[test]
    fn local_refs_and_unrelated_histories() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open_or_init(&tmp.path().join("r.git")).unwrap();
        assert_eq!(s.files_at_ref("refs/foreversync/disk/x").unwrap(), None);
        let files = set(&[("wtf/a.txt", "a")]);
        s.set_files_ref("refs/foreversync/disk/x", &files, "disk").unwrap();
        assert_eq!(s.files_at_ref("refs/foreversync/disk/x").unwrap(), Some(files));
        assert_eq!(s.head().unwrap(), None, "local refs don't touch main");

        let a = s.snapshot(&set(&[("x", "1")]), "a").unwrap().unwrap();
        let other = Store::open_or_init(&tmp.path().join("o.git")).unwrap();
        let o = other.snapshot(&set(&[("y", "1")]), "o").unwrap().unwrap();
        let remote = Remote { url: format!("file://{}", tmp.path().join("o.git").display()), token: None };
        assert_eq!(s.fetch(&remote).unwrap(), Some(o));
        assert_eq!(s.merge_base(a, o).unwrap(), None);
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
