//! Z.ai account usage adapter.
//!
//! Queries three Z.ai monitoring endpoints to gather quota windows,
//! model usage, and MCP tool breakdown.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ModelUsageSnapshot, ProviderAccountUsage, UsageSubItem, UsageWindow,
};

/// Z.ai account usage adapter. Queries Z.ai's monitoring API.
pub struct ZaiAccountUsage {
    provider_name: String,
    auth_token: String,
    base_url: String,
    agent: ureq::Agent,
}

impl ZaiAccountUsage {
    pub fn new(provider_name: String, auth_token: String, base_url: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        Self {
            provider_name,
            auth_token,
            base_url,
            agent,
        }
    }

    fn fetch_quota_limit(&self) -> Result<QuotaLimitResponse, ureq::Error> {
        let url = format!("{}/api/monitor/usage/quota/limit", self.base_url);
        let resp = self.agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?;
        Ok(resp.into_json()?)
    }

    fn fetch_model_usage(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<ModelUsageResponse, ureq::Error> {
        let url = format!(
            "{}/api/monitor/usage/model-usage?startTime={start_ms}&endTime={end_ms}",
            self.base_url
        );
        let resp = self.agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?;
        Ok(resp.into_json()?)
    }

    fn fetch_tool_usage(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<ToolUsageResponse, ureq::Error> {
        let url = format!(
            "{}/api/monitor/usage/tool-usage?startTime={start_ms}&endTime={end_ms}",
            self.base_url
        );
        let resp = self.agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?;
        Ok(resp.into_json()?)
    }
}

impl AccountUsagePort for ZaiAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let mut windows = Vec::new();
        let mut plan = None;
        let mut model_usage = None;

        // 1. Quota/limit endpoint (primary).
        match self.fetch_quota_limit() {
            Ok(data) => {
                plan = data.level;
                if let Some(limits) = data.limits {
                    let mut token_idx = 0;
                    for limit in &limits {
                        match limit.limit_type.as_str() {
                            "TOKENS_LIMIT" => {
                                let label = if token_idx == 0 {
                                    "5h Token"
                                } else {
                                    "Weekly"
                                };
                                token_idx += 1;
                                // Compute `used` from percentage + total if not provided directly.
                                let used = limit.used.or_else(|| {
                                    limit.total.map(|t| {
                                        ((limit.percentage.unwrap_or(0.0) / 100.0) * t as f64)
                                            as u64
                                    })
                                });
                                windows.push(UsageWindow {
                                    label: label.to_string(),
                                    used_pct: limit.percentage.unwrap_or(0.0),
                                    used,
                                    limit: limit.total,
                                    resets_at_ms: limit.next_reset_time,
                                    sub_items: vec![],
                                });
                            }
                            "TIME_LIMIT" => {
                                // For TIME_LIMIT: `usage` is the total limit,
                                // `current_value` is the used amount.
                                let mut sub_items = Vec::new();
                                if let Some(details) = &limit.usage_details {
                                    for d in details {
                                        sub_items.push(UsageSubItem {
                                            label: mcp_tool_label(&d.model_code),
                                            used: d.usage,
                                        });
                                    }
                                }
                                windows.push(UsageWindow {
                                    label: "MCP (1 Month)".to_string(),
                                    used_pct: limit.percentage.unwrap_or(0.0),
                                    used: limit.current_value,
                                    limit: limit.usage,
                                    resets_at_ms: limit.next_reset_time,
                                    sub_items,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("quota/limit: {e}"),
                }));
            }
        }

        // 2. Model usage (24h window). Failure here is non-fatal.
        let now_ms = now_epoch_millis();
        let start_ms = now_ms - 24 * 3600 * 1000;
        if let Ok(data) = self.fetch_model_usage(start_ms, now_ms) {
            if let Some(total) = data.total_usage {
                model_usage = Some(ModelUsageSnapshot {
                    total_tokens: total.total_tokens_usage.unwrap_or(0),
                    total_calls: total.total_model_call_count.unwrap_or(0),
                    period_start_ms: start_ms,
                    period_end_ms: now_ms,
                });
            }
        }

        // 3. Tool usage (24h window). Merge into MCP window's sub_items
        //    if the MCP window exists but has no sub_items.
        if let Ok(data) = self.fetch_tool_usage(start_ms, now_ms) {
            if let Some(total) = data.total_usage {
                if let Some(mcp) = windows.iter_mut().find(|w| w.label == "MCP (1 Month)") {
                    if mcp.sub_items.is_empty() {
                        if total.total_network_search_count > 0 {
                            mcp.sub_items.push(UsageSubItem {
                                label: "Network Searches".to_string(),
                                used: total.total_network_search_count,
                            });
                        }
                        if total.total_web_read_mcp_count > 0 {
                            mcp.sub_items.push(UsageSubItem {
                                label: "Web Reads".to_string(),
                                used: total.total_web_read_mcp_count,
                            });
                        }
                        if total.total_zread_mcp_count > 0 {
                            mcp.sub_items.push(UsageSubItem {
                                label: "ZRead Calls".to_string(),
                                used: total.total_zread_mcp_count,
                            });
                        }
                    }
                }
            }
        }

        Some(Ok(ProviderAccountUsage {
            provider: self.provider_name.clone(),
            status: AccountUsageStatus::Available,
            plan,
            windows,
            model_usage,
        }))
    }
}

fn mcp_tool_label(code: &str) -> String {
    match code {
        "search-prime" => "Network Searches".to_string(),
        "web-reader" => "Web Reads".to_string(),
        "zread" => "ZRead Calls".to_string(),
        other => other.to_string(),
    }
}

fn now_epoch_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ---------------------------------------------------------------------------
// Z.ai response JSON shapes (deserialized with serde)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct QuotaLimitResponse {
    level: Option<String>,
    limits: Option<Vec<QuotaLimitItem>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaLimitItem {
    #[serde(rename = "type")]
    limit_type: String,
    percentage: Option<f64>,
    total: Option<u64>,
    #[serde(default)]
    current_value: Option<u64>,
    #[serde(default)]
    used: Option<u64>,
    #[serde(default)]
    usage: Option<u64>,
    #[serde(default)]
    usage_details: Option<Vec<UsageDetail>>,
    next_reset_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageDetail {
    model_code: String,
    usage: u64,
}

#[derive(Debug, Deserialize)]
struct ModelUsageResponse {
    #[serde(rename = "totalUsage")]
    total_usage: Option<ModelTotalUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelTotalUsage {
    #[serde(default)]
    total_tokens_usage: Option<u64>,
    #[serde(default)]
    total_model_call_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ToolUsageResponse {
    #[serde(rename = "totalUsage")]
    total_usage: Option<ToolTotalUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolTotalUsage {
    #[serde(default)]
    total_network_search_count: u64,
    #[serde(default)]
    total_web_read_mcp_count: u64,
    #[serde(default)]
    total_zread_mcp_count: u64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_tool_label_maps_known_codes() {
        assert_eq!(mcp_tool_label("search-prime"), "Network Searches");
        assert_eq!(mcp_tool_label("web-reader"), "Web Reads");
        assert_eq!(mcp_tool_label("zread"), "ZRead Calls");
        assert_eq!(mcp_tool_label("other"), "other");
    }

    #[test]
    fn deserialize_quota_limit_response() {
        let json = r#"{
            "level": "pro",
            "limits": [
                {
                    "type": "TOKENS_LIMIT",
                    "percentage": 40.5,
                    "total": 40000000,
                    "nextResetTime": 1746300000000
                },
                {
                    "type": "TIME_LIMIT",
                    "percentage": 12.3,
                    "currentValue": 123,
                    "usage": 1000,
                    "usageDetails": [
                        {"modelCode": "search-prime", "usage": 5678}
                    ]
                }
            ]
        }"#;
        let resp: QuotaLimitResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.level.as_deref(), Some("pro"));
        let limits = resp.limits.unwrap();
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].limit_type, "TOKENS_LIMIT");
        assert_eq!(limits[0].percentage, Some(40.5));
        assert_eq!(limits[1].limit_type, "TIME_LIMIT");
        assert_eq!(limits[1].current_value, Some(123));
        assert_eq!(limits[1].usage, Some(1000));
    }

    #[test]
    fn deserialize_model_usage_response() {
        let json = r#"{
            "totalUsage": {
                "totalTokensUsage": 12500000,
                "totalModelCallCount": 1234
            }
        }"#;
        let resp: ModelUsageResponse = serde_json::from_str(json).unwrap();
        let total = resp.total_usage.unwrap();
        assert_eq!(total.total_tokens_usage, Some(12_500_000));
        assert_eq!(total.total_model_call_count, Some(1234));
    }

    #[test]
    fn deserialize_tool_usage_response() {
        let json = r#"{
            "totalUsage": {
                "totalNetworkSearchCount": 5678,
                "totalWebReadMcpCount": 2345,
                "totalZreadMcpCount": 890
            }
        }"#;
        let resp: ToolUsageResponse = serde_json::from_str(json).unwrap();
        let total = resp.total_usage.unwrap();
        assert_eq!(total.total_network_search_count, 5678);
        assert_eq!(total.total_web_read_mcp_count, 2345);
        assert_eq!(total.total_zread_mcp_count, 890);
    }
}
