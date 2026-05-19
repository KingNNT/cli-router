//! Background task that periodically refreshes OAuth tokens and persists
//! the refreshed tokens back to the config database.

use crate::application::ports::ConfigRepository;
use crate::config::AuthConfig;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::time;

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
        loop {
            interval.tick().await;
            if let Err(e) = refresh_expiring(&config, &config_repo, &http, &live).await {
                tracing::warn!(error = %e, "background token refresh sweep failed");
            }
        }
    })
}

enum OAuthKind {
    Anthropic,
    OpenAi,
    CodexAuto,
}

async fn refresh_expiring(
    config: &Arc<RwLock<crate::config::Config>>,
    config_repo: &Arc<dyn ConfigRepository>,
    http: &reqwest::Client,
    live: &Arc<super::LiveProvider>,
) -> Result<(), String> {
    // Collect providers that need refresh (name + old refresh token + kind).
    let to_refresh: Vec<(String, String, OAuthKind)> = {
        let cfg = config.read().map_err(|e| format!("config read: {e}"))?;
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

    for (name, old_rt, kind) in to_refresh {
        tracing::info!(provider = %name, "background refresh: refreshing OAuth token");

        let (access_token, refresh_token, expires_in) = match kind {
            OAuthKind::Anthropic => {
                let t = crate::adapters::oauth::anthropic::refresh_token(http, &old_rt)
                    .await
                    .map_err(|e| format!("refresh {name}: {e}"))?;
                (t.access_token, t.refresh_token, t.expires_in)
            }
            OAuthKind::OpenAi => {
                let t = crate::adapters::oauth::openai::refresh_token(http, &old_rt)
                    .await
                    .map_err(|e| format!("refresh {name}: {e}"))?;
                (t.access_token, t.refresh_token, t.expires_in)
            }
            OAuthKind::CodexAuto => {
                // Re-read ~/.codex/auth.json first — Codex CLI may have refreshed it.
                match crate::adapters::oauth::openai::read_auth_json() {
                    Ok(file_tokens) => {
                        let file_expires_in = file_tokens.expires_in.unwrap_or(0);
                        if file_expires_in > 300 {
                            // File tokens are fresh — use them, skip OAuth refresh.
                            tracing::info!(provider = %name, "CodexAuto: re-read fresh tokens from ~/.codex/auth.json");
                            (
                                file_tokens.access_token,
                                file_tokens.refresh_token,
                                file_tokens.expires_in,
                            )
                        } else {
                            // File tokens also expired — refresh using the file's refresh_token.
                            let rt = file_tokens.refresh_token.as_deref().unwrap_or("");
                            if rt.is_empty() {
                                tracing::warn!(provider = %name, "CodexAuto: ~/.codex/auth.json tokens expired and no refresh_token available");
                                continue;
                            }
                            tracing::info!(provider = %name, "CodexAuto: ~/.codex/auth.json tokens expired, refreshing");
                            let t = crate::adapters::oauth::openai::refresh_token(http, rt)
                                .await
                                .map_err(|e| format!("CodexAuto refresh {name}: {e}"))?;
                            (t.access_token, t.refresh_token, t.expires_in)
                        }
                    }
                    Err(e) => {
                        tracing::warn!(provider = %name, error = %e, "CodexAuto: failed to read ~/.codex/auth.json during refresh");
                        continue;
                    }
                }
            }
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let expires_at_ms = now_ms + expires_in.unwrap_or(3600) * 1000;

        // Update in-memory config.
        let new_cfg = {
            let mut cfg = config.write().map_err(|e| format!("config write: {e}"))?;
            if let Some(prov) = cfg.providers.iter_mut().find(|p| p.name == name) {
                prov.auth = match kind {
                    OAuthKind::Anthropic => AuthConfig::AnthropicOAuth {
                        access_token,
                        refresh_token: refresh_token.clone().unwrap_or(old_rt),
                        expires_at_ms,
                    },
                    OAuthKind::OpenAi => AuthConfig::OpenAiOAuth {
                        access_token,
                        refresh_token: refresh_token.clone().unwrap_or(old_rt),
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
        if !matches!(kind, OAuthKind::CodexAuto) {
            config_repo
                .save(&new_cfg)
                .map_err(|e| format!("save config: {e}"))?;
        }

        live.reload(&new_cfg, http.clone())
            .map_err(|e| format!("reload: {e}"))?;

        tracing::info!(provider = %name, "background refresh: OAuth token refreshed");
    }
    Ok(())
}
