//! GitHub integration (DESIGN.md §8): device-flow sign-in, token refresh and
//! storage, and the few REST calls setup needs. Git transport lives in
//! `foreversync-git`; it just needs the access token from here.

pub mod api;
pub mod auth;
pub mod config;
pub mod tokens;

pub use api::{Client, DataRepo, RepoInfo};
pub use auth::{DeviceCode, poll_device_flow, refresh, start_device_flow, valid_access_token};
pub use tokens::{FileStore, KeychainStore, TokenStore, Tokens};

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("HTTP error: {0}")]
    Http(#[from] ureq::Error),
    #[error("GitHub returned HTTP {status} for {what}: {body}")]
    Status { status: u16, what: String, body: String },
    #[error("sign-in failed: {0}")]
    Auth(String),
    #[error("not signed in")]
    NotSignedIn,
    #[error("token storage: {0}")]
    Storage(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, GithubError>;

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .user_agent(concat!("ForeverSync/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
