//! Device flow and token refresh. No client secret: device-flow tokens of a
//! GitHub App refresh with just the client id (verified 2026-11-18).

use std::time::Duration;

use serde::Deserialize;

use crate::config::{ACCESS_TOKEN_URL, CLIENT_ID, DEVICE_CODE_URL};
use crate::tokens::{TokenStore, Tokens};
use crate::{GithubError, Result, agent, now};

/// Refresh when the access token has less than this left.
const REFRESH_MARGIN_SECS: i64 = 5 * 60;

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    refresh_token_expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
    interval: Option<u64>,
}

impl TokenResponse {
    fn into_tokens(self) -> Option<Tokens> {
        let t = now();
        Some(Tokens {
            access_token: self.access_token?,
            access_expires_at: self.expires_in.map(|s| t + s),
            refresh_token: self.refresh_token,
            refresh_expires_at: self.refresh_token_expires_in.map(|s| t + s),
        })
    }
}

fn post_form(url: &str, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let mut resp = agent().post(url).header("Accept", "application/json").send_form(form.iter().copied())?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string()?;
    serde_json::from_str(&body).map_err(|_| GithubError::Status { status, what: url.into(), body })
}

/// Step 1: get a code for the user to enter at `verification_uri`.
pub fn start_device_flow() -> Result<DeviceCode> {
    let mut resp = agent()
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .send_form([("client_id", CLIENT_ID)])?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string()?;
    if status != 200 {
        return Err(GithubError::Status { status, what: "device code".into(), body });
    }
    serde_json::from_str(&body).map_err(|_| GithubError::Auth(format!("unexpected device-code reply: {body}")))
}

/// Step 2: wait until the user approves (or the code expires).
pub fn poll_device_flow(code: &DeviceCode) -> Result<Tokens> {
    let mut interval = code.interval.max(1);
    let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(interval));
        let r = post_form(
            ACCESS_TOKEN_URL,
            &[
                ("client_id", CLIENT_ID),
                ("device_code", &code.device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ],
        )?;
        match r.error.as_deref() {
            None => return r.into_tokens().ok_or_else(|| GithubError::Auth("no access token in reply".into())),
            Some("authorization_pending") => {}
            Some("slow_down") => interval = r.interval.unwrap_or(interval + 5),
            Some("expired_token") => return Err(GithubError::Auth("the code expired; start again".into())),
            Some("access_denied") => return Err(GithubError::Auth("authorization was denied".into())),
            Some(other) => {
                return Err(GithubError::Auth(format!("{other}: {}", r.error_description.unwrap_or_default())));
            }
        }
    }
    Err(GithubError::Auth("the code expired; start again".into()))
}

/// Exchanges a refresh token for a new pair. The old pair stops working.
pub fn refresh(refresh_token: &str) -> Result<Tokens> {
    let r = post_form(
        ACCESS_TOKEN_URL,
        &[("client_id", CLIENT_ID), ("grant_type", "refresh_token"), ("refresh_token", refresh_token)],
    )?;
    if let Some(e) = r.error {
        return Err(GithubError::Auth(format!("refresh failed: {e}: {}", r.error_description.unwrap_or_default())));
    }
    r.into_tokens().ok_or_else(|| GithubError::Auth("no access token in refresh reply".into()))
}

/// Returns a usable access token, refreshing (and saving the rotated pair
/// first) when it is about to expire.
pub fn valid_access_token(store: &dyn TokenStore) -> Result<String> {
    let tokens = store.load()?.ok_or(GithubError::NotSignedIn)?;
    if !tokens.access_expiring(now(), REFRESH_MARGIN_SECS) {
        return Ok(tokens.access_token);
    }
    let Some(rt) = tokens.refresh_token.as_deref() else { return Err(GithubError::NotSignedIn) };
    if tokens.refresh_expires_at.is_some_and(|t| t <= now()) {
        return Err(GithubError::NotSignedIn);
    }
    let fresh = refresh(rt)?;
    store.save(&fresh)?;
    Ok(fresh.access_token)
}
