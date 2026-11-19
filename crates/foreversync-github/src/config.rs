//! Public identifiers of the "Forever Sync" GitHub App. These are not secrets:
//! device flow needs only the client id, and the app has no client secret or
//! private key by design.

/// OAuth client id of the GitHub App (used for device flow and token refresh).
pub const CLIENT_ID: &str = "Iv23liujYZEO8HwwNyDn";

/// App slug: `https://github.com/apps/forever-sync`.
pub const APP_SLUG: &str = "forever-sync";

/// Default name of the user's private data repository.
pub const DEFAULT_DATA_REPO: &str = "forever-sync-data";

pub const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const API_BASE: &str = "https://api.github.com";

/// Page where the user installs the app on their data repo.
pub fn install_url() -> String {
    format!("https://github.com/apps/{APP_SLUG}/installations/new")
}

/// Prefilled "new repository" page used during first-machine onboarding.
pub fn new_repo_url() -> String {
    format!(
        "https://github.com/new?name={DEFAULT_DATA_REPO}&visibility=private&description=Forever%20Sync%20data%20%E2%80%94%20keep%20private"
    )
}
