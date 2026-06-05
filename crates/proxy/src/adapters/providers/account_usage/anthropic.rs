//! Anthropic OAuth account usage adapter.
//!
//! Hits Anthropic's undocumented `/api/oauth/usage` endpoint (the same one
//! Claude Code uses for its statusline). Returns the three utilization
//! windows (5h, 7d, 7d-opus) plus an optional extra-usage credit window.
//!
//! Caveats:
//! - Endpoint is reverse-engineered from Claude Code; Anthropic can change
//!   or remove it without notice.
//! - Aggressive 429s have been reported (see anthropic/claude-code#31021).
//!   We cache responses for 5 minutes and serve stale data on 429.
//! - Only OAuth tokens (`sk-ant-oat01-...`) work; API keys get 401.

use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::config::{AuthConfig, Config};
use crate::domain::account_usage::{AccountUsageStatus, ProviderAccountUsage, UsageWindow};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const BETA_HEADER: &str = "oauth-2025-04-20";
const CACHE_TTL: Duration = Duration::from_secs(300);
/// Profile data (plan tier, email) is essentially static — refresh hourly.
const PROFILE_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Anthropic OAuth account usage adapter.
pub struct AnthropicAccountUsage {
    provider_name: String,
    config: Arc<RwLock<Config>>,
    agent: ureq::Agent,
    cache: Mutex<Option<CacheEntry>>,
    /// Plan-tier cache. Separate from `cache` because it has a much longer
    /// TTL — the data is essentially static per-token.
    profile_cache: Mutex<Option<ProfileCacheEntry>>,
}

struct CacheEntry {
    fetched_at: Instant,
    value: ProviderAccountUsage,
}

struct ProfileCacheEntry {
    fetched_at: Instant,
    plan: Option<String>,
}

impl AnthropicAccountUsage {
    pub fn new(provider_name: String, config: Arc<RwLock<Config>>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        Self {
            provider_name,
            config,
            agent,
            cache: Mutex::new(None),
            profile_cache: Mutex::new(None),
        }
    }

    /// Read the current OAuth access token from the shared config.
    /// Returns `None` if the provider isn't configured with OAuth auth.
    fn current_token(&self) -> Option<String> {
        let cfg = self.config.read().ok()?;
        let prov = cfg
            .providers
            .iter()
            .find(|p| p.name == self.provider_name)?;
        match &prov.auth {
            AuthConfig::AnthropicOAuth { access_token, .. } => Some(access_token.clone()),
            _ => None,
        }
    }

    fn cached_fresh(&self) -> Option<ProviderAccountUsage> {
        let guard = self.cache.lock().ok()?;
        let entry = guard.as_ref()?;
        if entry.fetched_at.elapsed() < CACHE_TTL {
            Some(entry.value.clone())
        } else {
            None
        }
    }

    fn cached_any(&self) -> Option<ProviderAccountUsage> {
        let guard = self.cache.lock().ok()?;
        guard.as_ref().map(|e| e.value.clone())
    }

    fn store_cache(&self, value: ProviderAccountUsage) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some(CacheEntry {
                fetched_at: Instant::now(),
                value,
            });
        }
    }

    #[allow(clippy::result_large_err)]
    fn fetch_remote(&self, token: &str) -> Result<UsagePayload, ureq::Error> {
        let resp = self
            .agent
            .get(USAGE_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-beta", BETA_HEADER)
            .set("Accept", "application/json")
            .call()?;
        let payload: UsagePayload = resp.into_json()?;
        Ok(payload)
    }

    /// Fetch the profile (plan tier). Cheap and rarely changes — cached
    /// for 1 hour. Failure is non-fatal; we just don't show a plan label.
    fn cached_profile(&self) -> Option<Option<String>> {
        let guard = self.profile_cache.lock().ok()?;
        let entry = guard.as_ref()?;
        if entry.fetched_at.elapsed() < PROFILE_CACHE_TTL {
            Some(entry.plan.clone())
        } else {
            None
        }
    }

    fn store_profile_cache(&self, plan: Option<String>) {
        if let Ok(mut guard) = self.profile_cache.lock() {
            *guard = Some(ProfileCacheEntry {
                fetched_at: Instant::now(),
                plan,
            });
        }
    }

    #[allow(clippy::result_large_err)]
    fn fetch_profile(&self, token: &str) -> Result<ProfilePayload, ureq::Error> {
        let resp = self
            .agent
            .get(PROFILE_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/json")
            .call()?;
        let payload: ProfilePayload = resp.into_json()?;
        Ok(payload)
    }
}

impl AccountUsagePort for AnthropicAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        // Only OAuth-authenticated Anthropic providers have a usage endpoint.
        let token = self.current_token()?;

        // Serve usage from cache if fresh — protects the upstream from spammy
        // tab-clicks and dodges the 429s reported in claude-code#31021.
        if let Some(mut hit) = self.cached_fresh() {
            // Cheap to attach plan from cache too.
            if hit.plan.is_none()
                && let Some(plan) = self.cached_profile().flatten()
            {
                hit.plan = Some(plan);
            }
            return Some(Ok(hit));
        }

        // Fan out usage and profile in parallel. Profile is cached for 1h
        // so it's almost always a no-op; usage call dominates the latency.
        let (usage_result, profile_plan) = std::thread::scope(|s| {
            #[allow(clippy::result_large_err)]
            let u = s.spawn(|| self.fetch_remote(&token));
            let p = s.spawn(|| {
                if let Some(cached) = self.cached_profile() {
                    return cached;
                }
                match self.fetch_profile(&token) {
                    Ok(profile) => {
                        let plan = humanize_plan(profile.organization.rate_limit_tier.as_deref());
                        self.store_profile_cache(plan.clone());
                        plan
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "anthropic profile fetch failed (non-fatal)");
                        None
                    }
                }
            });
            (
                u.join().expect("usage thread panicked"),
                p.join().expect("profile thread panicked"),
            )
        });

        match usage_result {
            Ok(payload) => {
                let mut usage = payload_to_usage(self.provider_name.clone(), payload);
                usage.plan = profile_plan;
                self.store_cache(usage.clone());
                Some(Ok(usage))
            }
            Err(e) => {
                // On 429 (rate limited) fall back to whatever we have cached,
                // even if stale, rather than surfacing an error. This matches
                // the soft-failure guidance in the research notes.
                let is_429 = matches!(&e, ureq::Error::Status(429, _));
                if is_429 && let Some(mut stale) = self.cached_any() {
                    if stale.plan.is_none() {
                        stale.plan = profile_plan;
                    }
                    return Some(Ok(stale));
                }
                Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("oauth/usage: {e}"),
                }))
            }
        }
    }
}

/// Map Anthropic's internal `rate_limit_tier` codes to the labels the
/// claude.ai web UI shows. Unknown tiers fall back to the raw code so we
/// surface *something* rather than nothing if Anthropic introduces a new
/// plan we haven't mapped yet.
fn humanize_plan(tier: Option<&str>) -> Option<String> {
    let tier = tier?;
    let label = match tier {
        "default_claude_pro" => "Pro",
        "default_claude_max_5x" => "Max (5x)",
        "default_claude_max_20x" => "Max (20x)",
        "default_claude_team" => "Team",
        "default_claude_enterprise" => "Enterprise",
        other => other,
    };
    Some(label.to_string())
}

fn payload_to_usage(provider: String, p: UsagePayload) -> ProviderAccountUsage {
    let mut windows = Vec::new();

    // Plan-usage section: 5-hour rolling session window.
    if let Some(w) = p.five_hour {
        windows.push(window_from("Current Session".to_string(), w));
    }

    // Weekly section. Each subfield is null when the account doesn't
    // participate in that limit (e.g. a Sonnet-only plan returns
    // `seven_day_opus: null`). The omelette codename is Anthropic-internal
    // for the "Claude Design" weekly bucket.
    if let Some(w) = p.seven_day {
        windows.push(window_from("Weekly: All Models".to_string(), w));
    }
    if let Some(w) = p.seven_day_sonnet {
        windows.push(window_from("Weekly: Sonnet Only".to_string(), w));
    }
    if let Some(w) = p.seven_day_opus {
        windows.push(window_from("Weekly: Opus Only".to_string(), w));
    }
    if let Some(w) = p.seven_day_omelette {
        windows.push(window_from("Weekly: Claude Design".to_string(), w));
    }
    if let Some(w) = p.seven_day_oauth_apps {
        windows.push(window_from("Weekly: OAuth Apps".to_string(), w));
    }
    if let Some(w) = p.seven_day_cowork {
        windows.push(window_from("Weekly: Cowork".to_string(), w));
    }

    // Extra-usage credits (off-plan spend in USD).
    if let Some(eu) = p.extra_usage
        && eu.is_enabled
    {
        // Prefer Anthropic's pre-computed utilization if present;
        // fall back to dividing used by limit.
        let pct = eu
            .utilization
            .unwrap_or_else(|| match (eu.used_credits, eu.monthly_limit) {
                (Some(used), Some(limit)) if limit > 0.0 => (used / limit) * 100.0,
                _ => 0.0,
            });
        windows.push(UsageWindow {
            label: format!(
                "Extra Usage ({})",
                eu.currency.as_deref().unwrap_or("credits")
            ),
            used_pct: pct,
            used: eu.used_credits.map(|v| v as u64),
            limit: eu.monthly_limit.map(|v| v as u64),
            resets_at_ms: None,
            sub_items: vec![],
            is_balance_info: false,
        });
    }

    ProviderAccountUsage {
        provider,
        status: AccountUsageStatus::Available,
        plan: None,
        windows,
        model_usage: None,
    }
}

fn window_from(label: String, w: WindowField) -> UsageWindow {
    UsageWindow {
        label,
        used_pct: w.utilization,
        used: None,
        limit: None,
        resets_at_ms: w.resets_at.as_deref().and_then(parse_rfc3339_to_ms),
        sub_items: vec![],
        is_balance_info: false,
    }
}

/// Parse RFC 3339 / ISO 8601 timestamp to epoch milliseconds.
/// Returns None on parse failure rather than crashing.
fn parse_rfc3339_to_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Wire shape of `/api/oauth/usage`. Most fields can be `null` when the
/// account doesn't participate in that particular bucket — for example a
/// Sonnet-only Max plan returns `seven_day_opus: null`. Anthropic also
/// emits a few internal codenames (`omelette` = "Claude Design") and several
/// fields we don't have public mappings for yet (`tangelo`, `iguana_necktie`,
/// `omelette_promotional`); we ignore those silently.
#[derive(Debug, Deserialize)]
struct UsagePayload {
    #[serde(default)]
    five_hour: Option<WindowField>,
    #[serde(default)]
    seven_day: Option<WindowField>,
    #[serde(default)]
    seven_day_sonnet: Option<WindowField>,
    #[serde(default)]
    seven_day_opus: Option<WindowField>,
    #[serde(default)]
    seven_day_omelette: Option<WindowField>,
    #[serde(default)]
    seven_day_oauth_apps: Option<WindowField>,
    #[serde(default)]
    seven_day_cowork: Option<WindowField>,
    #[serde(default)]
    extra_usage: Option<ExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct WindowField {
    utilization: f64,
    #[serde(default)]
    resets_at: Option<String>,
}

/// Wire shape of `/api/oauth/profile`. We only care about the organization's
/// rate-limit tier — the rest of the payload (account uuid, email, etc.) is
/// ignored.
#[derive(Debug, Deserialize)]
struct ProfilePayload {
    organization: ProfileOrganization,
}

#[derive(Debug, Deserialize)]
struct ProfileOrganization {
    /// Codes observed in the wild: `default_claude_pro`, `default_claude_max_5x`,
    /// `default_claude_max_20x`. Marked optional so a payload missing the field
    /// (for free accounts? for new tiers?) deserializes successfully.
    #[serde(default)]
    rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExtraUsage {
    is_enabled: bool,
    /// Anthropic returns these as decimal numbers (e.g. `5928.0`, not `5928`).
    /// Deserialize as f64 and truncate when mapping to the integer DTO.
    #[serde(default)]
    used_credits: Option<f64>,
    #[serde(default)]
    monthly_limit: Option<f64>,
    /// Pre-computed utilization percentage (0–100). Present in real responses
    /// even though the early reverse-engineering notes didn't mention it.
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_full_payload() {
        let json = r#"{
            "five_hour":      { "utilization": 6.0,  "resets_at": "2025-11-04T04:59:59.943648+00:00" },
            "seven_day":      { "utilization": 35.0, "resets_at": "2025-11-06T03:59:59.943679+00:00" },
            "seven_day_opus": { "utilization": 0.0,  "resets_at": null },
            "extra_usage":    { "is_enabled": true, "used_credits": 500, "monthly_limit": 10000, "currency": "USD" }
        }"#;
        let p: UsagePayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.five_hour.as_ref().unwrap().utilization, 6.0);
        assert_eq!(p.seven_day.as_ref().unwrap().utilization, 35.0);
        assert!(p.seven_day_opus.is_some());
        let eu = p.extra_usage.unwrap();
        assert!(eu.is_enabled);
        assert_eq!(eu.used_credits, Some(500.0));
        assert_eq!(eu.monthly_limit, Some(10000.0));
        assert_eq!(eu.currency.as_deref(), Some("USD"));
    }

    #[test]
    fn deserialize_float_extra_usage_real_anthropic_shape() {
        // Real Anthropic responses send credits as floats (`5928.0`), not ints.
        // Regression test for the parse failure observed against the live API.
        let json = r#"{
            "five_hour":   { "utilization": 6.0,  "resets_at": "2025-11-04T04:59:59Z" },
            "extra_usage": { "is_enabled": true, "used_credits": 5928.0, "monthly_limit": 10000.0, "currency": "USD" }
        }"#;
        let mut p: UsagePayload = serde_json::from_str(json).unwrap();
        let eu = p.extra_usage.take().unwrap();
        assert_eq!(eu.used_credits, Some(5928.0));
        assert_eq!(eu.monthly_limit, Some(10000.0));
        // Re-parse to get a fresh payload (we consumed extra_usage above).
        let p: UsagePayload = serde_json::from_str(json).unwrap();
        let usage = payload_to_usage("Anthropic".into(), p);
        let extra = usage
            .windows
            .iter()
            .find(|w| w.label.starts_with("Extra Usage"))
            .unwrap();
        assert!((extra.used_pct - 59.28).abs() < 0.01);
        assert_eq!(extra.used, Some(5928));
        assert_eq!(extra.limit, Some(10000));
    }

    #[test]
    fn payload_to_usage_emits_all_known_windows_plus_extra() {
        let json = r#"{
            "five_hour":              { "utilization": 6.0,  "resets_at": "2025-11-04T04:59:59Z" },
            "seven_day":              { "utilization": 35.0, "resets_at": "2025-11-06T03:59:59Z" },
            "seven_day_sonnet":       { "utilization": 14.0, "resets_at": "2025-11-06T03:59:59Z" },
            "seven_day_opus":         { "utilization": 12.0, "resets_at": "2025-11-06T03:59:59Z" },
            "seven_day_omelette":     { "utilization": 0.0,  "resets_at": null },
            "extra_usage":            { "is_enabled": true, "used_credits": 500, "monthly_limit": 10000, "utilization": 5.0, "currency": "USD" }
        }"#;
        let p: UsagePayload = serde_json::from_str(json).unwrap();
        let usage = payload_to_usage("Anthropic".into(), p);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Current Session",
                "Weekly: All Models",
                "Weekly: Sonnet Only",
                "Weekly: Opus Only",
                "Weekly: Claude Design",
                "Extra Usage (USD)",
            ]
        );
        let extra = usage.windows.last().unwrap();
        assert_eq!(extra.used_pct, 5.0);
        assert_eq!(extra.used, Some(500));
        assert_eq!(extra.limit, Some(10000));
    }

    #[test]
    fn payload_to_usage_real_anthropic_max_response() {
        // Captured live from a Max(5x) account on 2026-05-05.
        let json = r#"{
            "five_hour":{"utilization":15.0,"resets_at":"2026-05-05T15:50:01.230244+00:00"},
            "seven_day":{"utilization":67.0,"resets_at":"2026-05-06T11:00:00.230262+00:00"},
            "seven_day_oauth_apps":null,
            "seven_day_opus":null,
            "seven_day_sonnet":{"utilization":14.0,"resets_at":"2026-05-06T11:00:00.230268+00:00"},
            "seven_day_cowork":null,
            "seven_day_omelette":{"utilization":0.0,"resets_at":null},
            "tangelo":null,
            "iguana_necktie":null,
            "omelette_promotional":null,
            "extra_usage":{"is_enabled":true,"monthly_limit":10000,"used_credits":5928.0,"utilization":59.28,"currency":"USD"}
        }"#;
        let p: UsagePayload = serde_json::from_str(json).unwrap();
        let usage = payload_to_usage("Anthropic".into(), p);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        // Null fields (opus, oauth_apps, cowork) are dropped; null-but-zero
        // omelette is kept because the account is enrolled.
        assert_eq!(
            labels,
            vec![
                "Current Session",
                "Weekly: All Models",
                "Weekly: Sonnet Only",
                "Weekly: Claude Design",
                "Extra Usage (USD)",
            ]
        );
        // Pre-computed utilization is preferred over recomputing from credits.
        let extra = usage.windows.last().unwrap();
        assert!((extra.used_pct - 59.28).abs() < 0.01);
        assert_eq!(extra.used, Some(5928));
    }

    #[test]
    fn payload_to_usage_hides_null_fields() {
        // Null subfields (the way Anthropic signals "not applicable") are omitted.
        let json = r#"{
            "five_hour":      { "utilization": 6.0,  "resets_at": "2025-11-04T04:59:59Z" },
            "seven_day":      { "utilization": 35.0, "resets_at": "2025-11-06T03:59:59Z" },
            "seven_day_opus": null
        }"#;
        let p: UsagePayload = serde_json::from_str(json).unwrap();
        let usage = payload_to_usage("Anthropic".into(), p);
        // Only 5h + 7d, no Opus, no Extra
        assert_eq!(usage.windows.len(), 2);
    }

    #[test]
    fn payload_to_usage_skips_disabled_extra_usage() {
        let json = r#"{
            "five_hour": { "utilization": 0.0, "resets_at": "2025-11-04T04:59:59Z" },
            "extra_usage": { "is_enabled": false }
        }"#;
        let p: UsagePayload = serde_json::from_str(json).unwrap();
        let usage = payload_to_usage("Anthropic".into(), p);
        assert_eq!(usage.windows.len(), 1);
    }

    #[test]
    fn humanize_plan_maps_known_tiers() {
        assert_eq!(
            humanize_plan(Some("default_claude_pro")).as_deref(),
            Some("Pro")
        );
        assert_eq!(
            humanize_plan(Some("default_claude_max_5x")).as_deref(),
            Some("Max (5x)")
        );
        assert_eq!(
            humanize_plan(Some("default_claude_max_20x")).as_deref(),
            Some("Max (20x)")
        );
        // Unknown tier falls through as-is rather than being dropped.
        assert_eq!(
            humanize_plan(Some("future_tier_xyz")).as_deref(),
            Some("future_tier_xyz")
        );
        assert!(humanize_plan(None).is_none());
    }

    #[test]
    fn deserialize_profile_payload() {
        // Captured shape from a real Max(5x) account.
        let json = r#"{
            "account": { "uuid": "...", "email": "x@y.z" },
            "organization": {
                "uuid": "...",
                "rate_limit_tier": "default_claude_max_5x",
                "has_extra_usage_enabled": true,
                "subscription_status": "active"
            },
            "application": { "uuid": "...", "name": "Claude Code" }
        }"#;
        let p: ProfilePayload = serde_json::from_str(json).unwrap();
        assert_eq!(
            p.organization.rate_limit_tier.as_deref(),
            Some("default_claude_max_5x")
        );
    }

    #[test]
    fn deserialize_profile_payload_missing_tier_is_ok() {
        // Free accounts may omit the field — must not break.
        let json = r#"{
            "organization": { "uuid": "..." }
        }"#;
        let p: ProfilePayload = serde_json::from_str(json).unwrap();
        assert!(p.organization.rate_limit_tier.is_none());
    }

    #[test]
    fn parse_rfc3339_to_ms_handles_z_and_offsets() {
        let a = parse_rfc3339_to_ms("2025-11-04T04:59:59Z").unwrap();
        let b = parse_rfc3339_to_ms("2025-11-04T04:59:59+00:00").unwrap();
        assert_eq!(a, b);
        assert!(parse_rfc3339_to_ms("not a date").is_none());
    }

    #[test]
    fn current_token_returns_none_for_non_oauth_auth() {
        use crate::config::{AffinityConfig, Config, ProviderConfig, ProviderKind, ThinkingMode};
        let cfg = Config {
            port: 8787,
            proxy_db: "".into(),
            pricing_db: "".into(),
            providers: vec![ProviderConfig {
                name: "anthropic".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::ApiKey { value: "k".into() },
                base_url: None,
                openai_base_url: None,
                reasoning_effort: None,
                thinking_mode: ThinkingMode::SplitOnly,
            }],
            routing: vec![],
            affinity: AffinityConfig::default(),
            quota: vec![],
        };
        let adapter = AnthropicAccountUsage::new("anthropic".into(), Arc::new(RwLock::new(cfg)));
        assert!(adapter.current_token().is_none());
    }

    #[test]
    fn current_token_reads_oauth_access_token() {
        use crate::config::{AffinityConfig, Config, ProviderConfig, ProviderKind, ThinkingMode};
        let cfg = Config {
            port: 8787,
            proxy_db: "".into(),
            pricing_db: "".into(),
            providers: vec![ProviderConfig {
                name: "Anthropic".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::AnthropicOAuth {
                    access_token: "sk-ant-oat01-fresh".into(),
                    refresh_token: "rt".into(),
                    expires_at_ms: 0,
                },
                base_url: None,
                openai_base_url: None,
                reasoning_effort: None,
                thinking_mode: ThinkingMode::SplitOnly,
            }],
            routing: vec![],
            affinity: AffinityConfig::default(),
            quota: vec![],
        };
        let adapter = AnthropicAccountUsage::new("Anthropic".into(), Arc::new(RwLock::new(cfg)));
        assert_eq!(
            adapter.current_token().as_deref(),
            Some("sk-ant-oat01-fresh")
        );
    }

    #[test]
    fn fetch_usage_returns_none_when_not_oauth() {
        // No OAuth token configured → adapter must declare "not supported"
        // via None, NOT report an error.
        use crate::config::{AffinityConfig, Config, ProviderConfig, ProviderKind, ThinkingMode};
        let cfg = Config {
            port: 8787,
            proxy_db: "".into(),
            pricing_db: "".into(),
            providers: vec![ProviderConfig {
                name: "anthropic".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::ApiKey { value: "k".into() },
                base_url: None,
                openai_base_url: None,
                reasoning_effort: None,
                thinking_mode: ThinkingMode::SplitOnly,
            }],
            routing: vec![],
            affinity: AffinityConfig::default(),
            quota: vec![],
        };
        let adapter = AnthropicAccountUsage::new("anthropic".into(), Arc::new(RwLock::new(cfg)));
        assert!(adapter.fetch_usage().is_none());
    }
}
