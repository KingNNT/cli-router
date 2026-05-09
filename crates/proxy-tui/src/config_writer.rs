//! Dual-mode config read/write for the TUI.
//!
//! When the proxy isn't running, the TUI can fall back to reading/writing the
//! TOML config file directly.  The path resolution mirrors the proxy's logic
//! in `crates/proxy/src/config.rs`.

use proxy_admin_api::ConfigPayload;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read a TOML file and deserialize into [`ConfigPayload`].
pub fn read_from_file(path: &Path) -> Result<ConfigPayload, String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    toml::from_str(&contents).map_err(|e| format!("failed to parse TOML: {e}"))
}

/// Serialize [`ConfigPayload`] to TOML and write to disk.
///
/// Parent directories are created automatically if they don't exist.
pub fn write_to_file(config: &ConfigPayload, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create dirs for {}: {e}", parent.display()))?;
    }
    let contents =
        toml::to_string_pretty(config).map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(path, contents)
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

/// Resolve the config file path, mirroring the proxy's `Config::resolved_path`.
///
/// Priority:
/// 1. `CLI_ROUTER_CONFIG` env var (explicit override)
/// 2. `CLI_ROUTER_PROFILE` → profile-specific filename under the config dir
/// 3. `~/.config/cli-router/config.toml` (XDG default)
#[allow(dead_code)]
pub fn resolved_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CLI_ROUTER_CONFIG") {
        return PathBuf::from(path);
    }
    let profile = std::env::var("CLI_ROUTER_PROFILE").ok();
    dirs_config_dir().join(config_filename_for_profile(profile.as_deref()))
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn dirs_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".config")
        })
        .join("cli-router")
}

fn config_filename_for_profile(profile: Option<&str>) -> &'static str {
    match profile {
        Some("dev") | Some("development") => "config.dev.toml",
        Some(other) if !matches!(other, "" | "prod" | "production") => {
            // Mirrors the proxy: unknown profile falls back to prod config
            "config.toml"
        }
        _ => "config.toml",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proxy_admin_api::*;

    fn sample_payload() -> ConfigPayload {
        ConfigPayload {
            port: 8787,
            providers: vec![ProviderPayload {
                name: "test-provider".into(),
                kind: "anthropic".into(),
                auth: AuthPayload::Passthrough,
                base_url: None,
                openai_base_url: None,
            }],
            routing: vec![],
            quota: vec![],
            affinity: AffinityPayload {
                enabled: true,
                headers: vec!["x-session-id".into()],
            },
            proxy_db: None,
            pricing_db: None,
        }
    }

    #[test]
    fn roundtrip_toml_file() {
        let dir = std::env::temp_dir().join("cli-router-test-roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");

        let original = sample_payload();
        write_to_file(&original, &path).unwrap();
        let loaded = read_from_file(&path).unwrap();

        assert_eq!(loaded.port, original.port);
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].name, "test-provider");
        assert!(loaded.affinity.enabled);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_from_missing_file_returns_error() {
        let path = std::env::temp_dir().join("nonexistent-config-xyz.toml");
        let result = read_from_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn write_creates_parent_dirs() {
        let dir = std::env::temp_dir().join("cli-router-test-mkdirs").join("nested");
        let path = dir.join("config.toml");

        let payload = sample_payload();
        write_to_file(&payload, &path).unwrap();
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(
            std::env::temp_dir().join("cli-router-test-mkdirs"),
        );
    }
}
