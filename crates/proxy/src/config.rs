//! Env-var-driven proxy configuration.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub proxy_db: PathBuf,
    pub pricing_db: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let port = std::env::var("CLI_ROUTER_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8787);
        let proxy_db = std::env::var_os("CLI_ROUTER_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share/cli-router/proxy.db"));
        let pricing_db = std::env::var_os("CLI_ROUTER_PRICING_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share/cli-router/pricing.db"));
        Config {
            port,
            proxy_db,
            pricing_db,
        }
    }
}
