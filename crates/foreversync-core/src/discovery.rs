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

/// Finds the Forever install: `explicit` root if given, otherwise the default
/// install roots for this OS. (Reading Battle.net's product.db comes later.)
pub fn discover(explicit: Option<&Path>) -> Result<ForeverInstall, DiscoveryError> {
    if let Some(root) = explicit {
        return open(root);
    }
    let roots = forever::default_install_roots();
    for root in roots {
        let root = Path::new(root);
        if root.join(forever::FLAVOR_DIR).is_dir() {
            return open(root);
        }
    }
    Err(DiscoveryError::NotFound(roots.join(", ")))
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
