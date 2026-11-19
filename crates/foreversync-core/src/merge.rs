//! Per-file 3-way merge (DESIGN.md §7.3). Never merges file *contents*:
//! a line merge of Lua tables can produce files WoW discards.

use serde::Serialize;

use crate::FileSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conflict {
    pub path: String,
    /// Which side's version was kept. The other stays in history.
    pub kept: Side,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeResult {
    pub files: FileSet,
    pub conflicts: Vec<Conflict>,
}

/// Merges `local` and `remote` against their common ancestor `base` (empty
/// for unrelated histories). When both changed a file differently, `prefer`
/// wins, except that a change always beats a deletion.
pub fn merge3(base: &FileSet, local: &FileSet, remote: &FileSet, prefer: Side) -> MergeResult {
    let mut files = FileSet::new();
    let mut conflicts = Vec::new();
    let paths: std::collections::BTreeSet<&String> = base.keys().chain(local.keys()).chain(remote.keys()).collect();

    for path in paths {
        let (b, l, r) = (base.get(path), local.get(path), remote.get(path));
        let chosen = if l == r {
            l
        } else if l == b {
            r
        } else if r == b {
            l
        } else {
            match (l, r) {
                (None, Some(_)) => r, // local deleted, remote changed: keep the change
                (Some(_), None) => l,
                _ => {
                    conflicts.push(Conflict { path: path.clone(), kept: prefer });
                    if prefer == Side::Local { l } else { r }
                }
            }
        };
        if let Some(bytes) = chosen {
            files.insert(path.clone(), bytes.clone());
        }
    }
    MergeResult { files, conflicts }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, &str)]) -> FileSet {
        pairs.iter().map(|(p, c)| (p.to_string(), c.as_bytes().to_vec())).collect()
    }

    #[test]
    fn decision_table() {
        let base = set(&[
            ("same", "0"),
            ("local_only", "0"),
            ("remote_only", "0"),
            ("both_same", "0"),
            ("both_diff", "0"),
            ("del_local", "0"),
            ("del_remote", "0"),
            ("del_local_chg_remote", "0"),
            ("chg_local_del_remote", "0"),
            ("del_both", "0"),
        ]);
        let local = set(&[
            ("same", "0"),
            ("local_only", "L"),
            ("remote_only", "0"),
            ("both_same", "X"),
            ("both_diff", "L"),
            ("del_remote", "0"),
            ("chg_local_del_remote", "L"),
            ("new_local", "L"),
            ("new_both", "L"),
        ]);
        let remote = set(&[
            ("same", "0"),
            ("local_only", "0"),
            ("remote_only", "R"),
            ("both_same", "X"),
            ("both_diff", "R"),
            ("del_local", "0"),
            ("del_local_chg_remote", "R"),
            ("new_remote", "R"),
            ("new_both", "R"),
        ]);
        let m = merge3(&base, &local, &remote, Side::Remote);
        let expect = set(&[
            ("same", "0"),
            ("local_only", "L"),
            ("remote_only", "R"),
            ("both_same", "X"),
            ("both_diff", "R"),
            ("del_local_chg_remote", "R"),
            ("chg_local_del_remote", "L"),
            ("new_local", "L"),
            ("new_remote", "R"),
            ("new_both", "R"),
        ]);
        assert_eq!(m.files, expect);
        let c: Vec<_> = m.conflicts.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(c, ["both_diff", "new_both"]);

        let m = merge3(&base, &local, &remote, Side::Local);
        assert_eq!(m.files.get("both_diff").unwrap(), b"L");
        assert_eq!(m.files.get("new_both").unwrap(), b"L");
    }

    #[test]
    fn unrelated_histories_union() {
        let m = merge3(&FileSet::new(), &set(&[("a", "1"), ("c", "L")]), &set(&[("b", "2"), ("c", "R")]), Side::Local);
        assert_eq!(m.files, set(&[("a", "1"), ("b", "2"), ("c", "L")]));
        assert_eq!(m.conflicts.len(), 1);
    }
}
