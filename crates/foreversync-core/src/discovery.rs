//! Locating and describing a WoW Forever install (DESIGN.md §3.1). Read-only.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::forever;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("no WoW Forever install found (looked in: {0})")]
    NotFound(String),
    #[error("{path} is not a WoW Forever install: {reason}")]
    NotForever { path: PathBuf, reason: String },
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ForeverInstall {
    /// Install root, e.g. `/Applications/World of Warcraft`.
    pub root: PathBuf,
    /// The Forever client folder, `<root>/_forever_`.
    pub flavor_dir: PathBuf,
    /// Client version from `.build.info`, e.g. `1.60.1.70938`.
    pub version: Option<String>,
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub realms: Vec<Realm>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Realm {
    /// Forever uses numeric realm ids as folder names (e.g. `93`).
    pub id: String,
    pub characters: Vec<Character>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Character {
    pub folder: String,
}

impl ForeverInstall {
    pub fn wtf_dir(&self) -> PathBuf {
        self.flavor_dir.join("WTF")
    }

    pub fn addons_dir(&self) -> PathBuf {
        self.flavor_dir.join("Interface").join("AddOns")
    }
}

/// Finds the Forever install: `explicit` root if given; otherwise install
/// roots listed in Battle.net's `product.db`, then this OS's default roots.
pub fn discover(explicit: Option<&Path>) -> Result<ForeverInstall, DiscoveryError> {
    if let Some(root) = explicit {
        return open(root);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for db in forever::battle_net_product_dbs() {
        if let Ok(bytes) = fs::read(&db) {
            candidates.extend(roots_from_product_db(&bytes));
        }
    }
    candidates.extend(forever::default_install_roots().iter().map(PathBuf::from));

    let mut seen = std::collections::HashSet::new();
    for root in candidates.iter().filter(|r| seen.insert((*r).clone())) {
        if root.join(forever::FLAVOR_DIR).is_dir() {
            if let Ok(install) = open(root) {
                return Ok(install);
            }
        }
    }
    let looked: Vec<String> = seen.iter().map(|p| p.display().to_string()).collect();
    Err(DiscoveryError::NotFound(if looked.is_empty() { "nowhere".into() } else { looked.join(", ") }))
}

/// Extracts absolute paths from Battle.net's `product.db`.
///
/// The file is protobuf. Rather than depend on its undocumented schema, walk
/// the wire format generically, collect every string field, and keep those
/// that are absolute paths (`/Applications/...`, `E:/Games/...`,
/// `E:\\Games\\...`). Callers then check each for a `_forever_` folder.
pub fn roots_from_product_db(bytes: &[u8]) -> Vec<PathBuf> {
    let mut strings = Vec::new();
    walk_protobuf(bytes, 0, &mut strings);
    let mut out: Vec<PathBuf> = Vec::new();
    for s in strings {
        let b = s.as_bytes();
        let unix = b.first() == Some(&b'/') && b.get(1).is_some_and(|c| c.is_ascii_alphanumeric());
        let windows = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\');
        if !(unix || windows) || s.chars().any(char::is_control) {
            continue;
        }
        let p = PathBuf::from(s.trim_end_matches(['/', '\\']));
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Collects UTF-8 string fields from protobuf-encoded `buf`, recursing into
/// length-delimited fields that themselves parse as messages. Returns false
/// if `buf` is not a well-formed message.
fn walk_protobuf(buf: &[u8], depth: u32, strings: &mut Vec<String>) -> bool {
    fn varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *buf.get(*pos)?;
            *pos += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    if depth > 16 {
        return false;
    }
    let mut found = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let Some(key) = varint(buf, &mut pos) else { return false };
        if key >> 3 == 0 {
            return false;
        }
        match key & 7 {
            0 => {
                if varint(buf, &mut pos).is_none() {
                    return false;
                }
            }
            1 => pos += 8,
            5 => pos += 4,
            2 => {
                let Some(len) = varint(buf, &mut pos) else { return false };
                let Some(end) = pos.checked_add(len as usize).filter(|e| *e <= buf.len()) else { return false };
                let field = &buf[pos..end];
                if let Ok(s) = std::str::from_utf8(field) {
                    found.push(s.to_owned());
                }
                if !field.is_empty() {
                    walk_protobuf(field, depth + 1, &mut found);
                }
                pos = end;
            }
            _ => return false,
        }
        if pos > buf.len() {
            return false;
        }
    }
    strings.extend(found);
    true
}

/// Opens and validates a specific install root.
pub fn open(root: &Path) -> Result<ForeverInstall, DiscoveryError> {
    let flavor_dir = root.join(forever::FLAVOR_DIR);
    if !flavor_dir.is_dir() {
        return Err(DiscoveryError::NotForever {
            path: root.to_path_buf(),
            reason: format!("missing {} folder", forever::FLAVOR_DIR),
        });
    }

    let flavor_info = flavor_dir.join(forever::FLAVOR_INFO_FILE);
    match read_flavor_product(&flavor_info) {
        Some(p) if p == forever::PRODUCT_CODE => {}
        Some(p) => {
            return Err(DiscoveryError::NotForever {
                path: root.to_path_buf(),
                reason: format!("{} says product `{p}`", forever::FLAVOR_INFO_FILE),
            });
        }
        // Missing/unreadable .flavor.info: the folder name is good enough.
        None => {}
    }

    let version = fs::read_to_string(root.join(forever::BUILD_INFO_FILE))
        .ok()
        .and_then(|s| build_info_version(&s, forever::PRODUCT_CODE));

    let accounts = list_accounts(&flavor_dir.join("WTF").join("Account"))?;

    Ok(ForeverInstall {
        root: root.to_path_buf(),
        flavor_dir,
        version,
        accounts,
    })
}

/// `.flavor.info` is a one-column table: a header line, then the product code.
fn read_flavor_product(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines()
        .skip(1)
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

/// Parses Blizzard's pipe-separated `.build.info` table and returns the
/// `Version` of the row whose `Product` equals `product`.
pub fn build_info_version(text: &str, product: &str) -> Option<String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<&str> = lines
        .next()?
        .split('|')
        .map(|col| col.split('!').next().unwrap_or(col))
        .collect();
    let product_col = header.iter().position(|c| *c == "Product")?;
    let version_col = header.iter().position(|c| *c == "Version")?;
    lines.find_map(|line| {
        let cells: Vec<&str> = line.split('|').collect();
        (cells.get(product_col)?.trim() == product)
            .then(|| cells.get(version_col).map(|v| v.trim().to_owned()))
            .flatten()
            .filter(|v| !v.is_empty())
    })
}

fn list_accounts(account_root: &Path) -> Result<Vec<Account>, DiscoveryError> {
    sorted_subdirs(account_root)?
        .into_iter()
        .filter(|name| name != forever::SAVED_VARIABLES_DIR)
        .map(|name| {
            let dir = account_root.join(&name);
            let realms = sorted_subdirs(&dir)?
                .into_iter()
                .filter(|r| r != forever::SAVED_VARIABLES_DIR)
                .map(|id| {
                    let characters = sorted_subdirs(&dir.join(&id))?
                        .into_iter()
                        .map(|folder| Character { folder })
                        .collect();
                    Ok(Realm { id, characters })
                })
                .collect::<Result<_, DiscoveryError>>()?;
            Ok(Account { name, realms })
        })
        .collect()
}

/// Names of the immediate subdirectories of `dir`, sorted. A missing `dir`
/// yields an empty list (a fresh install has no `WTF/Account` yet).
fn sorted_subdirs(dir: &Path) -> Result<Vec<String>, DiscoveryError> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(DiscoveryError::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD_INFO: &str = "Branch!STRING:0|Active!DEC:1|Version!STRING:0|Product!STRING:0\n\
                              eu|1|1.60.1.70938|wowf\n\
                              eu|1|12.0.7.1|wow\n";

    #[test]
    fn parses_build_info_version_for_product() {
        assert_eq!(build_info_version(BUILD_INFO, "wowf").as_deref(), Some("1.60.1.70938"));
        assert_eq!(build_info_version(BUILD_INFO, "wow").as_deref(), Some("12.0.7.1"));
        assert_eq!(build_info_version(BUILD_INFO, "wow_classic"), None);
        assert_eq!(build_info_version("", "wowf"), None);
    }

    fn mkdirs(root: &Path, rel: &[&str]) {
        for r in rel {
            fs::create_dir_all(root.join(r)).unwrap();
        }
    }

    #[test]
    fn opens_fixture_install() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        mkdirs(
            root,
            &[
                "_forever_/WTF/Account/SavedVariables",
                "_forever_/WTF/Account/TESTACCT/SavedVariables",
                "_forever_/WTF/Account/TESTACCT/93/Alpha-One",
                "_forever_/WTF/Account/TESTACCT/93/Beta-Two",
                "_forever_/Interface/AddOns",
            ],
        );
        fs::write(root.join(".build.info"), BUILD_INFO).unwrap();
        fs::write(root.join("_forever_/.flavor.info"), "Product Flavor!STRING:0\nwowf\n").unwrap();

        let install = open(root).unwrap();
        assert_eq!(install.version.as_deref(), Some("1.60.1.70938"));
        assert_eq!(install.accounts.len(), 1);
        let acct = &install.accounts[0];
        assert_eq!(acct.name, "TESTACCT");
        assert_eq!(acct.realms.len(), 1);
        assert_eq!(acct.realms[0].id, "93");
        let chars: Vec<_> = acct.realms[0].characters.iter().map(|c| c.folder.as_str()).collect();
        assert_eq!(chars, ["Alpha-One", "Beta-Two"]);
        assert_eq!(install.addons_dir(), root.join("_forever_/Interface/AddOns"));
    }

    /// Minimal protobuf encoder for building product.db-shaped test data.
    fn field(num: u64, payload: &[u8]) -> Vec<u8> {
        fn varint(mut v: u64, out: &mut Vec<u8>) {
            while v >= 0x80 {
                out.push((v as u8) | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        }
        let mut out = Vec::new();
        varint((num << 3) | 2, &mut out);
        varint(payload.len() as u64, &mut out);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn extracts_install_paths_from_product_db() {
        let install = |code: &str, path: &str| {
            let settings = [field(1, path.as_bytes()), field(4, b"_forever_"), field(2, b"enUS")].concat();
            field(1, &[field(1, code.as_bytes()), field(2, code.as_bytes()), field(3, &settings)].concat())
        };
        let db = [
            install("wowf", "E:/Games/World of Warcraft/"),
            install("wowf", "/Applications/World of Warcraft"),
            install("agent", "C:\\Program Files (x86)\\Battle.net"),
            install("wowf", "D:\\Games\\World of Warcraft"),
            field(2, b"not a path"),
            vec![0x08, 0x96, 0x01], // a varint field
        ]
        .concat();
        let roots: Vec<_> = roots_from_product_db(&db).iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(
            roots,
            [
                "E:/Games/World of Warcraft",
                "/Applications/World of Warcraft",
                "C:\\Program Files (x86)\\Battle.net",
                "D:\\Games\\World of Warcraft",
            ]
        );
        assert!(roots_from_product_db(b"\xff\xff garbage").is_empty());
        assert!(roots_from_product_db(b"").is_empty());
    }

    /// Reads this machine's real Battle.net product.db; run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_product_db_lists_an_install() {
        for db in forever::battle_net_product_dbs() {
            let Ok(bytes) = fs::read(&db) else { continue };
            let roots = roots_from_product_db(&bytes);
            println!("{}: {roots:?}", db.display());
            assert!(roots.iter().any(|r| r.join(forever::FLAVOR_DIR).is_dir()), "no _forever_ root in {roots:?}");
        }
    }

    #[test]
    fn rejects_other_flavor() {
        let tmp = tempfile::tempdir().unwrap();
        mkdirs(tmp.path(), &["_forever_"]);
        fs::write(tmp.path().join("_forever_/.flavor.info"), "Product Flavor!STRING:0\nwow\n").unwrap();
        assert!(matches!(open(tmp.path()), Err(DiscoveryError::NotForever { .. })));
    }

    #[test]
    fn missing_flavor_dir_is_not_forever() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(open(tmp.path()), Err(DiscoveryError::NotForever { .. })));
    }
}
