//! Anthropic OAuth (PKCE) — paste-flow variant.
//!
//! Anthropic does not publish a third-party OAuth integration; we reuse the
//! public `client_id` shipped in Claude Code (`9d1c250a-…`). The flow:
//!
//! 1. Daemon generates a PKCE verifier + challenge + state.
//! 2. Caller opens the resulting authorization URL in a browser.
//! 3. After login Anthropic redirects to the manual-callback page where it
//!    displays `code#state` for the user to copy.
//! 4. Caller submits `code` to `complete_oauth` — we exchange for tokens at
//!    `console.anthropic.com/v1/oauth/token` and return the access token.
//!
//! Risk: the `client_id` is intended for Claude Code, not third-party
//! proxies. Anthropic may rotate or restrict it without notice. If that
//! happens, fall back to manual API-key paste or `claude setup-token`.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
pub const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
pub const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
pub const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers";

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("unknown state — start a new flow")]
    UnknownState,
    #[error("token exchange transport: {0}")]
    Transport(String),
    #[error("token exchange status {0}: {1}")]
    Upstream(u16, String),
    #[error("token response decode: {0}")]
    Decode(String),
}

#[derive(Debug, Clone)]
pub struct PkceCodes {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

/// Build a PKCE verifier (RFC 7636: 43-128 chars from the unreserved set)
/// from two stitched UUIDs base64-url-encoded with no padding.
pub fn generate_pkce() -> PkceCodes {
    let raw = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes());
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(h.finalize());
    let state = Uuid::new_v4().to_string();
    PkceCodes {
        verifier,
        challenge,
        state,
    }
}

pub fn build_authorize_url(codes: &PkceCodes) -> String {
    // Manual encoding to avoid pulling in url crate; everything we put in is
    // already URL-safe.
    let scopes_enc = SCOPES.replace(' ', "%20").replace(':', "%3A");
    let redirect_enc = REDIRECT_URI.replace(':', "%3A").replace('/', "%2F");
    format!(
        "{AUTHORIZE_URL}?\
         response_type=code&\
         client_id={CLIENT_ID}&\
         redirect_uri={redirect_enc}&\
         scope={scopes_enc}&\
         state={state}&\
         code_challenge={challenge}&\
         code_challenge_method=S256",
        state = codes.state,
        challenge = codes.challenge,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

pub async fn exchange_code(
    http: &reqwest::Client,
    code: &str,
    verifier: &str,
) -> Result<OAuthTokens, OAuthError> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "client_id": CLIENT_ID,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    });
    let resp = http
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| OAuthError::Transport(e.to_string()))?;
    let status = resp.status().as_u16();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| OAuthError::Transport(e.to_string()))?;
    if !(200..300).contains(&status) {
        let msg = String::from_utf8_lossy(&bytes).to_string();
        return Err(OAuthError::Upstream(status, msg));
    }
    let parsed: TokenResponse =
        serde_json::from_slice(&bytes).map_err(|e| OAuthError::Decode(e.to_string()))?;
    Ok(OAuthTokens {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_in: parsed.expires_in,
    })
}

/// Refresh an expired access token using a refresh token. Returns a new
/// `OAuthTokens` with a fresh access token and (typically) a rotated refresh
/// token. If the response omits `refresh_token`, callers should keep the old
/// one (per OAuth best practice).
pub async fn refresh_token(
    http: &reqwest::Client,
    old_refresh_token: &str,
) -> Result<OAuthTokens, OAuthError> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": old_refresh_token,
    });
    let resp = http
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| OAuthError::Transport(e.to_string()))?;
    let status = resp.status().as_u16();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| OAuthError::Transport(e.to_string()))?;
    if !(200..300).contains(&status) {
        let msg = String::from_utf8_lossy(&bytes).to_string();
        return Err(OAuthError::Upstream(status, msg));
    }
    let parsed: TokenResponse =
        serde_json::from_slice(&bytes).map_err(|e| OAuthError::Decode(e.to_string()))?;
    Ok(OAuthTokens {
        access_token: parsed.access_token,
        // If the server didn't rotate, keep the old refresh token.
        refresh_token: parsed
            .refresh_token
            .or_else(|| Some(old_refresh_token.to_string())),
        expires_in: parsed.expires_in,
    })
}

/// In-memory ledger of pending OAuth flows. Keyed by `state_id`. Survives
/// only as long as the daemon — restarts wipe pending flows. Sized at 32 max
/// to bound memory if a buggy client spams `start`.
#[derive(Default)]
pub struct OAuthSessionStore {
    inner: Mutex<HashMap<String, PkceCodes>>,
}

impl OAuthSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, codes: PkceCodes) {
        let mut g = self.inner.lock().expect("oauth sessions mutex poisoned");
        if g.len() >= 32 {
            // Drop an arbitrary old entry to make room — nothing here is
            // critical, the worst case is the user has to start over.
            if let Some(stale) = g.keys().next().cloned() {
                g.remove(&stale);
            }
        }
        g.insert(codes.state.clone(), codes);
    }

    pub fn take(&self, state_id: &str) -> Option<PkceCodes> {
        let mut g = self.inner.lock().expect("oauth sessions mutex poisoned");
        g.remove(state_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_is_43_chars_or_more() {
        let c = generate_pkce();
        assert!(c.verifier.len() >= 43);
        assert!(c.verifier.len() <= 128);
    }

    #[test]
    fn pkce_challenge_is_distinct_from_verifier() {
        let c = generate_pkce();
        assert_ne!(c.verifier, c.challenge);
    }

    #[test]
    fn pkce_state_is_uuid_shape() {
        let c = generate_pkce();
        assert_eq!(c.state.len(), 36); // uuid v4 dashed
    }

    #[test]
    fn authorize_url_contains_required_params() {
        let c = generate_pkce();
        let url = build_authorize_url(&c);
        assert!(url.starts_with("https://claude.ai/oauth/authorize?"));
        assert!(url.contains(&format!("state={}", c.state)));
        assert!(url.contains(&format!("code_challenge={}", c.challenge)));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains(CLIENT_ID));
    }

    #[test]
    fn session_store_round_trip() {
        let s = OAuthSessionStore::new();
        let c = generate_pkce();
        let state = c.state.clone();
        s.insert(c);
        let taken = s.take(&state).unwrap();
        assert_eq!(taken.state, state);
        // Second take is None (consumed).
        assert!(s.take(&state).is_none());
    }

    #[test]
    fn session_store_caps_at_32_entries() {
        let s = OAuthSessionStore::new();
        for _ in 0..40 {
            s.insert(generate_pkce());
        }
        let g = s.inner.lock().unwrap();
        assert!(g.len() <= 32);
    }
}
