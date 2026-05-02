//! Background task that periodically refreshes OAuth tokens and persists
//! the refreshed tokens back to the config file.
//!
//! This is the **primary** token refresh mechanism. The inline check in
//! `messages_protocol::forward` only handles the 401-retry safety net;
//! proactive refresh happens here so that refreshed tokens are always
//! persisted to disk immediately (avoiding refresh-token rotation races).

use crate::config::AuthConfig;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::time;

/// Spawn a background task that checks OAuth token expiry every 60 seconds
/// and refreshes any that are within 5 minutes of expiring. Refreshed tokens
/// are written back to the config file and the live provider tree is rebuilt.
pub fn spawn(
    config: Arc<RwLock<crate::config::Config>>,
    config_path: PathBuf,
    http: reqwest::Client,
    live: Arc<super::LiveProvider>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = refresh_expiring(&config, &config_path, &http, &live).await {
                tracing::warn!(error = %e, "background token refresh sweep failed");
            }
        }
    })
}

async fn refresh_expiring(
    config: &Arc<RwLock<crate::config::Config>>,
    config_path: &PathBuf,
    http: &reqwest::Client,
    live: &Arc<super::LiveProvider>,
) -> Result<(), String> {
    // Collect providers that need refresh (name + old refresh token).
    let to_refresh: Vec<(String, String)> = {
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
                    Some((p.name.clone(), refresh_token.clone()))
                }
                _ => None,
            })
            .collect()
    };

    for (name, old_rt) in to_refresh {
        tracing::info!(provider = %name, "background refresh: refreshing OAuth token");
        let tokens = crate::adapters::oauth::refresh_token(http, &old_rt)
            .await
            .map_err(|e| format!("refresh {name}: {e}"))?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let expires_at_ms = now_ms + tokens.expires_in.unwrap_or(3600) * 1000;

        // Update in-memory config.
        let new_cfg = {
            let mut cfg = config.write().map_err(|e| format!("config write: {e}"))?;
            if let Some(prov) = cfg.providers.iter_mut().find(|p| p.name == name) {
                prov.auth = AuthConfig::AnthropicOAuth {
                    access_token: tokens.access_token.clone(),
                    refresh_token: tokens
                        .refresh_token
                        .clone()
                        .unwrap_or(old_rt),
                    expires_at_ms,
                };
            }
            cfg.clone()
        }; // Write lock dropped here.

        // Persist to disk and rebuild provider tree (outside the lock).
        // NOTE: This replaces any ${VAR} placeholders with their resolved
        // values. This is a known limitation shared with CompleteAnthropicOAuth.
        if let Some(parent) = config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let toml_str = toml::to_string_pretty(&new_cfg)
            .map_err(|e| format!("serialize: {e}"))?;
        std::fs::write(config_path, toml_str)
            .map_err(|e| format!("write: {e}"))?;

        live.reload(&new_cfg, http.clone())
            .map_err(|e| format!("reload: {e}"))?;

        tracing::info!(provider = %name, "background refresh: OAuth token refreshed and persisted");
    }
    Ok(())
}
