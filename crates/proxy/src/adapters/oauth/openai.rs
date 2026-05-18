//! OpenAI Codex auth — reads the cached token from `codex login`.
//!
//! Codex CLI (`codex login`) handles authentication via browser or device
//! code flow and caches tokens at `~/.codex/auth.json`. We read that file
//! directly instead of implementing our own OAuth flow, because OpenAI's
//! Codex auth uses a device-code flow (not a standard PKCE web redirect)
//! and the client_id is tied to the Codex CLI binary.
//!
//! Usage:
//! 1. User runs `codex login` once to authenticate with ChatGPT
//! 2. The proxy reads `~/.codex/auth.json` to get the access token
//! 3. Token is used as `Authorization: Bearer <token>` for API calls
//! 4. Codex CLI handles background token refresh automatically

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Auth JSON structure as produced by `codex login`.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CodexAuthFile {
    #[serde(default)]
    auth_mode: Option<String>,
    #[serde(default)]
    tokens: Option<CodexTokens>,
    #[serde(default)]
    last_refresh: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("auth.json not found at {0} — run `codex login` first")]
    NotFound(String),
    #[error("auth.json parse: {0}")]
    Parse(String),
    #[error("auth.json has no access_token")]
    MissingToken,
    #[error("JWT decode: {0}")]
    JwtDecode(String),
}

#[derive(Debug, Clone)]
pub struct PkceCodes {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
}

/// In-memory session store. No longer used for PKCE; kept for API compatibility.
#[derive(Default)]
pub struct OAuthSessionStore {
    #[allow(dead_code)]
    inner: std::sync::Mutex<std::collections::HashMap<String, PkceCodes>>,
}

impl OAuthSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, _codes: PkceCodes) {}
    pub fn take(&self, _state_id: &str) -> Option<PkceCodes> {
        None
    }
}

/// Path to the Codex auth cache file.
fn auth_json_path() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    home.join(".codex").join("auth.json")
}

/// Read access + refresh tokens from `~/.codex/auth.json`.
pub fn read_auth_json() -> Result<OAuthTokens, OAuthError> {
    let path = auth_json_path();
    let raw = std::fs::read_to_string(&path)
        .map_err(|_| OAuthError::NotFound(path.display().to_string()))?;
    let af: CodexAuthFile =
        serde_json::from_str(&raw).map_err(|e| OAuthError::Parse(e.to_string()))?;

    let tokens = af.tokens.ok_or(OAuthError::MissingToken)?;
    let access_token = tokens.access_token.ok_or(OAuthError::MissingToken)?;

    // Decode JWT `exp` to compute `expires_in` relative to now.
    let expires_in = decode_jwt_exp(&access_token).ok();

    Ok(OAuthTokens {
        access_token,
        refresh_token: tokens.refresh_token,
        expires_in,
    })
}

/// Try to extract the `exp` claim from a JWT (without verifying signature).
fn decode_jwt_exp(jwt: &str) -> Result<u64, OAuthError> {
    let payload = jwt
        .split('.')
        .nth(1)
        .ok_or_else(|| OAuthError::JwtDecode("not a JWT (no segments)".into()))?;

    // Add padding if needed for base64 decoding.
    let padded = match payload.len() % 4 {
        0 => payload.to_string(),
        n => format!("{}{}", payload, "=".repeat(4 - n)),
    };

    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&padded)
        .map_err(|e| OAuthError::JwtDecode(format!("base64: {e}")))?;

    let claims: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|e| OAuthError::JwtDecode(format!("json: {e}")))?;

    claims["exp"]
        .as_u64()
        .map(|exp| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            exp.saturating_sub(now)
        })
        .ok_or_else(|| OAuthError::JwtDecode("no exp claim".into()))
}

/// Dummy PKCE generator — kept for API compatibility with anthropic pattern.
/// The OpenAi OAuth no longer uses PKCE; we read tokens from auth.json.
pub fn generate_pkce() -> PkceCodes {
    PkceCodes {
        verifier: String::new(),
        challenge: String::new(),
        state: String::new(),
    }
}

/// The authorization URL is no longer used — the user runs `codex login` separately.
pub fn build_authorize_url(_codes: &PkceCodes) -> String {
    String::new()
}

/// Dummy exchange — kept for API compatibility.
pub async fn exchange_code(
    _http: &reqwest::Client,
    _code: &str,
    _state: &str,
    _verifier: &str,
) -> Result<OAuthTokens, OAuthError> {
    read_auth_json()
}

/// Refresh is handled by `codex` CLI updating auth.json. We just re-read.
pub async fn refresh_token(
    _http: &reqwest::Client,
    _old_refresh_token: &str,
) -> Result<OAuthTokens, OAuthError> {
    read_auth_json()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn decode_exp_from_valid_jwt() {
        // A JWT with exp=2000000000 (roughly May 2033)
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"exp":2000000000}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let result = decode_jwt_exp(&jwt);
        assert!(result.is_ok());
        assert!(result.unwrap() > 0); // exp is in the future
    }

    #[test]
    fn decode_exp_missing_payload() {
        let result = decode_jwt_exp("not-a-jwt");
        assert!(result.is_err());
    }

    #[test]
    fn decode_exp_no_exp_claim() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"user"}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let result = decode_jwt_exp(&jwt);
        assert!(result.is_err());
    }

    #[test]
    fn session_store_is_noop() {
        let s = OAuthSessionStore::new();
        // No-ops — kept for API compatibility
        s.insert(generate_pkce());
        assert!(s.take("any").is_none());
    }
}
