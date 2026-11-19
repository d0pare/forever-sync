//! User access token pair and where it is kept.
//!
//! Each refresh **rotates** both tokens and invalidates the old pair (verified
//! 2026-11-18), so a new pair must be saved before it is used.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{GithubError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    /// Unix seconds; `None` if the app has token expiry turned off.
    pub access_expires_at: Option<i64>,
    pub refresh_token: Option<String>,
    pub refresh_expires_at: Option<i64>,
}

impl Tokens {
    /// True if the access token expires within `margin_secs`.
    pub fn access_expiring(&self, now: i64, margin_secs: i64) -> bool {
        self.access_expires_at.is_some_and(|t| t - margin_secs <= now)
    }
}

pub trait TokenStore {
    fn load(&self) -> Result<Option<Tokens>>;
    fn save(&self, tokens: &Tokens) -> Result<()>;
    fn clear(&self) -> Result<()>;
}

/// macOS Keychain / Windows Credential Manager.
pub struct KeychainStore {
    service: String,
    user: String,
}

impl Default for KeychainStore {
    fn default() -> Self {
        Self { service: "Forever Sync".into(), user: "github".into() }
    }
}

impl KeychainStore {
    /// A store under a custom service name (tests, side-by-side builds).
    pub fn new(service: &str, user: &str) -> Self {
        Self { service: service.into(), user: user.into() }
    }

    fn entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &self.user).map_err(|e| GithubError::Storage(e.to_string()))
    }
}

impl TokenStore for KeychainStore {
    fn load(&self) -> Result<Option<Tokens>> {
        match self.entry()?.get_password() {
            Ok(s) => Ok(Some(serde_json::from_str(&s)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(GithubError::Storage(e.to_string())),
        }
    }
    fn save(&self, tokens: &Tokens) -> Result<()> {
        self.entry()?
            .set_password(&serde_json::to_string(tokens)?)
            .map_err(|e| GithubError::Storage(e.to_string()))
    }
    fn clear(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(GithubError::Storage(e.to_string())),
        }
    }
}

/// A JSON file readable only by the user. For development and tests.
pub struct FileStore(pub PathBuf);

impl TokenStore for FileStore {
    fn load(&self) -> Result<Option<Tokens>> {
        match fs::read_to_string(&self.0) {
            Ok(s) => Ok(Some(serde_json::from_str(&s)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(GithubError::Storage(e.to_string())),
        }
    }
    fn save(&self, tokens: &Tokens) -> Result<()> {
        let err = |e: std::io::Error| GithubError::Storage(e.to_string());
        if let Some(dir) = self.0.parent() {
            fs::create_dir_all(dir).map_err(err)?;
        }
        let tmp = self.0.with_extension("tmp");
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            let mut f = opts.open(&tmp).map_err(err)?;
            std::io::Write::write_all(&mut f, serde_json::to_string(tokens)?.as_bytes()).map_err(err)?;
            f.sync_all().map_err(err)?;
        }
        fs::rename(&tmp, &self.0).map_err(err)
    }
    fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.0) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(GithubError::Storage(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Touches the real OS keychain; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let store = KeychainStore::new("Forever Sync Test", "github-test");
        let t = Tokens { access_token: "ghu_test".into(), access_expires_at: None, refresh_token: None, refresh_expires_at: None };
        store.save(&t).unwrap();
        assert_eq!(store.load().unwrap(), Some(t));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn file_store_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileStore(tmp.path().join("t/tokens.json"));
        assert_eq!(store.load().unwrap(), None);
        let t = Tokens {
            access_token: "ghu_x".into(),
            access_expires_at: Some(100),
            refresh_token: Some("ghr_y".into()),
            refresh_expires_at: Some(1000),
        };
        store.save(&t).unwrap();
        assert_eq!(store.load().unwrap(), Some(t.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&store.0).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert!(t.access_expiring(50, 60));
        assert!(!t.access_expiring(10, 60));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}
