//! Background task that periodically refreshes OAuth tokens and persists
//! the refreshed tokens back to the config database.

use crate::application::ports::ConfigRepository;
use crate::config::AuthConfig;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::time;

/// Smallest transient-failure backoff. Doubles on each consecutive transient
/// failure up to [`BACKOFF_MAX`].
const BACKOFF_BASE: Duration = Duration::from_secs(60);
/// Cap on the transient-failure backoff so we keep probing roughly twice an
/// hour even during a prolonged upstream outage.
const BACKOFF_MAX: Duration = Duration::from_secs(1800);

/// Spawn a background task that checks OAuth token expiry every 60 seconds
/// and refreshes any that are within 5 minutes of expiring. Refreshed tokens
/// are written back to the config DB and the live provider tree is rebuilt.
pub fn spawn(
    config: Arc<RwLock<crate::config::Config>>,
    config_repo: Arc<dyn ConfigRepository>,
    http: reqwest::Client,
    live: Arc<super::LiveProvider>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        let mut state = RefreshState::default();
        loop {
            interval.tick().await;
            refresh_expiring(&config, &config_repo, &http, &live, &mut state).await;
        }
    })
}

enum OAuthKind {
    Anthropic,
    OpenAi,
    CodexAuto,
}

/// Per-provider refresh health, carried across sweeps so a permanently broken
/// refresh token doesn't get retried every 60s forever (which spams the log
/// and rate-limits the token endpoint).
#[derive(Default)]
struct RefreshState {
    /// provider name → the refresh-token value that the upstream has
    /// permanently rejected (`invalid_grant`). While the *configured* refresh
    /// token still equals this value, refresh is paused — retrying a dead
    /// token can never succeed. Re-authenticating changes the token, which
    /// clears the pause automatically (the values no longer match).
    dead: HashMap<String, String>,
    /// provider name → instant before which we must not retry (transient
    /// failures get exponential backoff so we don't hammer a struggling
    /// upstream).
    backoff_until: HashMap<String, Instant>,
    /// provider name → consecutive transient-failure count (drives the
    /// exponential backoff delay).
    transient_fails: HashMap<String, u32>,
}

impl RefreshState {
    /// True if this provider's refresh should be skipped this sweep — either
    /// its refresh token is known-dead and unchanged, or it's still inside a
    /// transient-failure backoff window.
    fn should_skip(&self, name: &str, current_refresh_token: &str, now: Instant) -> bool {
        if let Some(dead_tok) = self.dead.get(name)
            && dead_tok == current_refresh_token
        {
            return true;
        }
        if let Some(until) = self.backoff_until.get(name)
            && now < *until
        {
            return true;
        }
        false
    }

    /// Clear all failure bookkeeping for a provider after a successful refresh.
    fn record_success(&mut self, name: &str) {
        self.dead.remove(name);
        self.backoff_until.remove(name);
        self.transient_fails.remove(name);
    }

    /// Mark a refresh token as permanently rejected. Refresh is paused until
    /// the configured token changes (i.e. the user re-authenticates).
    fn record_terminal(&mut self, name: &str, refresh_token: &str) {
        self.dead
            .insert(name.to_string(), refresh_token.to_string());
        self.backoff_until.remove(name);
        self.transient_fails.remove(name);
    }

    /// Record a transient failure, scheduling the next allowed attempt with
    /// exponential backoff from [`BACKOFF_BASE`] up to [`BACKOFF_MAX`].
    fn record_transient(&mut self, name: &str, now: Instant) {
        let fails = self.transient_fails.entry(name.to_string()).or_insert(0);
        let delay = backoff_delay(*fails);
        *fails = fails.saturating_add(1);
        self.backoff_until.insert(name.to_string(), now + delay);
    }
}

/// Exponential backoff: `BACKOFF_BASE * 2^fails`, capped at `BACKOFF_MAX`.
fn backoff_delay(fails: u32) -> Duration {
    let mult = 1u64.checked_shl(fails.min(16)).unwrap_or(u64::MAX);
    BACKOFF_BASE
        .checked_mul(mult as u32)
        .unwrap_or(BACKOFF_MAX)
        .min(BACKOFF_MAX)
}

/// How a failed refresh should be treated.
enum FailKind {
    /// Refresh token permanently rejected — pause until re-auth.
    Terminal(String),
    /// Temporary failure — back off and retry.
    Transient(String),
}

/// Classify an Anthropic OAuth refresh error. A `400`/`401` (typically
/// `invalid_grant`) means the refresh token is permanently dead; everything
/// else (rate limits, 5xx, transport) is transient.
fn classify_anthropic(e: &crate::adapters::oauth::anthropic::OAuthError) -> FailKind {
    use crate::adapters::oauth::anthropic::OAuthError;
    match e {
        OAuthError::Upstream(status, body) if matches!(status, 400 | 401) => {
            FailKind::Terminal(format!("status {status}: {body}"))
        }
        other => FailKind::Transient(other.to_string()),
    }
}

async fn refresh_expiring(
    config: &Arc<RwLock<crate::config::Config>>,
    config_repo: &Arc<dyn ConfigRepository>,
    http: &reqwest::Client,
    live: &Arc<super::LiveProvider>,
    state: &mut RefreshState,
) {
    // Collect providers that need refresh (name + old refresh token + kind).
    let to_refresh: Vec<(String, String, OAuthKind)> = {
        let cfg = match config.read() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "token refresh: config read failed");
                return;
            }
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        cfg.providers
            .iter()
            .filter_map(|p| match &p.auth {
                AuthConfig::AnthropicOAuth {
                    refresh_token,
                    expires_at_ms,
                    ..
                } if now_ms + 300_000 >= *expires_at_ms && !refresh_token.is_empty() => {
                    Some((p.name.clone(), refresh_token.clone(), OAuthKind::Anthropic))
                }
                AuthConfig::OpenAiOAuth {
                    refresh_token,
                    expires_at_ms,
                    ..
                } if now_ms + 300_000 >= *expires_at_ms && !refresh_token.is_empty() => {
                    Some((p.name.clone(), refresh_token.clone(), OAuthKind::OpenAi))
                }
                AuthConfig::CodexAuto => {
                    // Always re-read the file to check for fresh tokens.
                    Some((p.name.clone(), String::new(), OAuthKind::CodexAuto))
                }
                _ => None,
            })
            .collect()
    };

    let now = Instant::now();
    for (name, old_rt, kind) in to_refresh {
        // Skip providers whose refresh token is known-dead (until re-auth) or
        // that are still inside a transient-failure backoff window.
        if state.should_skip(&name, &old_rt, now) {
            continue;
        }

        tracing::info!(provider = %name, "background refresh: refreshing OAuth token");

        let refreshed = match kind {
            OAuthKind::Anthropic => {
                match crate::adapters::oauth::anthropic::refresh_token(http, &old_rt).await {
                    Ok(t) => Ok((t.access_token, t.refresh_token, t.expires_in)),
                    Err(e) => Err(classify_anthropic(&e)),
                }
            }
            OAuthKind::OpenAi => {
                match crate::adapters::oauth::openai::refresh_token(http, &old_rt).await {
                    Ok(t) => Ok((t.access_token, t.refresh_token, t.expires_in)),
                    // The OpenAI/OAuth refresh path reads local state; treat
                    // failures as transient (file may briefly be missing).
                    Err(e) => Err(FailKind::Transient(e.to_string())),
                }
            }
            OAuthKind::CodexAuto => {
                // Re-read ~/.codex/auth.json first — Codex CLI may have refreshed it.
                match crate::adapters::oauth::openai::read_auth_json() {
                    Ok(file_tokens) => {
                        let file_expires_in = file_tokens.expires_in.unwrap_or(0);
                        if file_expires_in > 300 {
                            // File tokens are fresh — use them, skip OAuth refresh.
                            tracing::info!(provider = %name, "CodexAuto: re-read fresh tokens from ~/.codex/auth.json");
                            Ok((
                                file_tokens.access_token,
                                file_tokens.refresh_token,
                                file_tokens.expires_in,
                            ))
                        } else {
                            // File tokens also expired — refresh using the file's refresh_token.
                            let rt = file_tokens.refresh_token.as_deref().unwrap_or("");
                            if rt.is_empty() {
                                tracing::warn!(provider = %name, "CodexAuto: ~/.codex/auth.json tokens expired and no refresh_token available");
                                continue;
                            }
                            tracing::info!(provider = %name, "CodexAuto: ~/.codex/auth.json tokens expired, refreshing");
                            match crate::adapters::oauth::openai::refresh_token(http, rt).await {
                                Ok(t) => Ok((t.access_token, t.refresh_token, t.expires_in)),
                                Err(e) => Err(FailKind::Transient(e.to_string())),
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(provider = %name, error = %e, "CodexAuto: failed to read ~/.codex/auth.json during refresh");
                        continue;
                    }
                }
            }
        };

        let (access_token, refresh_token, expires_in) = match refreshed {
            Ok(tokens) => tokens,
            Err(FailKind::Terminal(detail)) => {
                tracing::warn!(
                    provider = %name,
                    detail = %detail,
                    "background refresh: refresh token rejected (needs re-authentication) — \
                     pausing refresh for this provider until it is re-authenticated via OAuth"
                );
                state.record_terminal(&name, &old_rt);
                continue;
            }
            Err(FailKind::Transient(detail)) => {
                state.record_transient(&name, now);
                let until = state.backoff_until.get(&name).copied();
                tracing::warn!(
                    provider = %name,
                    detail = %detail,
                    backoff = ?until.map(|u| u.saturating_duration_since(now)),
                    "background refresh: transient failure — backing off"
                );
                continue;
            }
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let expires_at_ms = now_ms + expires_in.unwrap_or(3600) * 1000;

        // Update in-memory config.
        let new_cfg = {
            let mut cfg = match config.write() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "token refresh: config write failed");
                    return;
                }
            };
            if let Some(prov) = cfg.providers.iter_mut().find(|p| p.name == name) {
                prov.auth = match kind {
                    OAuthKind::Anthropic => AuthConfig::AnthropicOAuth {
                        access_token,
                        refresh_token: refresh_token.clone().unwrap_or(old_rt.clone()),
                        expires_at_ms,
                    },
                    OAuthKind::OpenAi => AuthConfig::OpenAiOAuth {
                        access_token,
                        refresh_token: refresh_token.clone().unwrap_or(old_rt.clone()),
                        expires_at_ms,
                    },
                    OAuthKind::CodexAuto => AuthConfig::OpenAiOAuth {
                        access_token,
                        refresh_token: refresh_token.clone().unwrap_or_default(),
                        expires_at_ms,
                    },
                };
            }
            cfg.clone()
        }; // Write lock dropped here.

        // Persist to DB and rebuild provider tree — skip for CodexAuto
        // (CodexAuto never writes tokens to config).
        if !matches!(kind, OAuthKind::CodexAuto)
            && let Err(e) = config_repo.save(&new_cfg)
        {
            tracing::warn!(provider = %name, error = %e, "token refresh: save config failed");
            continue;
        }

        if let Err(e) = live.reload(&new_cfg, http.clone()) {
            tracing::warn!(provider = %name, error = %e, "token refresh: provider reload failed");
            continue;
        }

        state.record_success(&name);
        tracing::info!(provider = %name, "background refresh: OAuth token refreshed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::oauth::anthropic::OAuthError;

    #[test]
    fn invalid_grant_is_terminal() {
        let e = OAuthError::Upstream(
            400,
            r#"{"error":"invalid_grant","error_description":"Refresh token not found or invalid"}"#
                .into(),
        );
        assert!(matches!(classify_anthropic(&e), FailKind::Terminal(_)));
    }

    #[test]
    fn unauthorized_is_terminal() {
        let e = OAuthError::Upstream(401, "unauthorized".into());
        assert!(matches!(classify_anthropic(&e), FailKind::Terminal(_)));
    }

    #[test]
    fn rate_limit_is_transient() {
        let e = OAuthError::Upstream(429, "rate limited".into());
        assert!(matches!(classify_anthropic(&e), FailKind::Transient(_)));
    }

    #[test]
    fn server_error_is_transient() {
        let e = OAuthError::Upstream(503, "unavailable".into());
        assert!(matches!(classify_anthropic(&e), FailKind::Transient(_)));
    }

    #[test]
    fn transport_error_is_transient() {
        let e = OAuthError::Transport("connection reset".into());
        assert!(matches!(classify_anthropic(&e), FailKind::Transient(_)));
    }

    #[test]
    fn dead_token_is_skipped_until_it_changes() {
        let mut state = RefreshState::default();
        let now = Instant::now();
        state.record_terminal("claude", "dead-rt");
        // Same (dead) refresh token → skip.
        assert!(state.should_skip("claude", "dead-rt", now));
        // A new refresh token (re-auth) → no longer skipped.
        assert!(!state.should_skip("claude", "fresh-rt", now));
    }

    #[test]
    fn success_clears_dead_state() {
        let mut state = RefreshState::default();
        let now = Instant::now();
        state.record_terminal("claude", "dead-rt");
        state.record_success("claude");
        assert!(!state.should_skip("claude", "dead-rt", now));
    }

    #[test]
    fn transient_failure_backs_off_then_clears() {
        let mut state = RefreshState::default();
        let now = Instant::now();
        state.record_transient("claude", now);
        // Immediately after, still inside the backoff window.
        assert!(state.should_skip("claude", "rt", now));
        // Well past the max backoff, retry is allowed again.
        assert!(!state.should_skip("claude", "rt", now + BACKOFF_MAX + Duration::from_secs(1)));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_delay(0), BACKOFF_BASE);
        assert_eq!(backoff_delay(1), BACKOFF_BASE * 2);
        assert_eq!(backoff_delay(2), BACKOFF_BASE * 4);
        // Large failure counts saturate at the cap rather than overflowing.
        assert_eq!(backoff_delay(64), BACKOFF_MAX);
    }
}
