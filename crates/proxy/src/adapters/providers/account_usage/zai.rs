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

    #[allow(clippy::result_large_err)]
    fn fetch_quota_limit(&self) -> Result<QuotaLimitData, ureq::Error> {
        let url = format!("{}/api/monitor/usage/quota/limit", self.base_url);
        let resp = self
            .agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?;
        let envelope: ZaiEnvelope<QuotaLimitData> = resp.into_json()?;
        Ok(envelope.data)
    }

    #[allow(clippy::result_large_err)]
    fn fetch_model_usage(&self, start_ms: i64, end_ms: i64) -> Result<ModelUsageData, ureq::Error> {
        let url = format!(
            "{}/api/monitor/usage/model-usage?startTime={start_ms}&endTime={end_ms}",
            self.base_url
        );
        let resp = self
            .agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?;
        let envelope: ModelUsageEnvelope = resp.into_json()?;
        Ok(envelope.data)
    }

    #[allow(clippy::result_large_err)]
    fn fetch_tool_usage(&self, start_ms: i64, end_ms: i64) -> Result<ToolUsageData, ureq::Error> {
        let url = format!(
            "{}/api/monitor/usage/tool-usage?startTime={start_ms}&endTime={end_ms}",
            self.base_url
        );
        let resp = self
            .agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?;
        let envelope: ToolUsageEnvelope = resp.into_json()?;
        Ok(envelope.data)
    }
}

impl AccountUsagePort for ZaiAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let mut windows = Vec::new();
        #[allow(unused_assignments)]
        let mut plan = None;
        let mut model_usage = None;

        // Fan out the three monitor calls in parallel — they're independent
        // and Z.ai responses can take ~1s each. Sequential = 3× the latency.
        let now_ms = now_epoch_millis();
        let start_ms = now_ms - 24 * 3600 * 1000;
        let (quota_result, model_result, tool_result) = std::thread::scope(|s| {
            // The fetch_* helpers already #[allow] result_large_err for ureq::Error;
            // propagate that allow to the spawned closures so clippy stays clean.
            #[allow(clippy::result_large_err)]
            let q = s.spawn(|| self.fetch_quota_limit());
            #[allow(clippy::result_large_err)]
            let m = s.spawn(|| self.fetch_model_usage(start_ms, now_ms));
            #[allow(clippy::result_large_err)]
            let t = s.spawn(|| self.fetch_tool_usage(start_ms, now_ms));
            (
                q.join().expect("quota thread panicked"),
                m.join().expect("model-usage thread panicked"),
                t.join().expect("tool-usage thread panicked"),
            )
        });

        // 1. Quota/limit endpoint (primary).
        match quota_result {
            Ok(data) => {
                plan = data.level;
                if let Some(limits) = data.limits {
                    // Z.ai returns limits in arbitrary order.
                    // TOKENS_LIMIT with unit=3 (hours) = 5h window.
                    // TOKENS_LIMIT with unit=6 (days) = weekly window.
                    // TIME_LIMIT = MCP monthly window.
                    let mut five_hour: Option<UsageWindow> = None;
                    let mut weekly: Option<UsageWindow> = None;
                    let mut mcp: Option<UsageWindow> = None;

                    for limit in &limits {
                        match limit.limit_type.as_str() {
                            "TOKENS_LIMIT" => {
                                // Z.ai labels: unit=3 → hourly window (5h plan window),
                                // unit=6 → weekly window. Match the web dashboard wording.
                                let (label, is_five_hour) = match (limit.unit, limit.number) {
                                    (Some(3), Some(n)) if n <= 12 => {
                                        ("5 Hours Quota".to_string(), n == 5)
                                    }
                                    (Some(6), _) => ("Weekly Quota".to_string(), false),
                                    _ => {
                                        // Fallback: use index-based naming.
                                        let is_first = five_hour.is_none();
                                        let label = if is_first {
                                            "5 Hours Quota"
                                        } else {
                                            "Weekly Quota"
                                        };
                                        (label.to_string(), is_first)
                                    }
                                };

                                let used = limit.used.or_else(|| {
                                    limit.total.map(|t| {
                                        ((limit.percentage.unwrap_or(0.0) / 100.0) * t as f64)
                                            as u64
                                    })
                                });

                                let window = UsageWindow {
                                    label,
                                    used_pct: limit.percentage.unwrap_or(0.0),
                                    used,
                                    limit: limit.total,
                                    resets_at_ms: limit.next_reset_time,
                                    sub_items: vec![],
                                };

                                if is_five_hour {
                                    five_hour = Some(window);
                                } else {
                                    weekly = Some(window);
                                }
                            }
                            "TIME_LIMIT" => {
                                let mut sub_items = Vec::new();
                                if let Some(details) = &limit.usage_details {
                                    for d in details {
                                        sub_items.push(UsageSubItem {
                                            label: mcp_tool_label(&d.model_code),
                                            used: d.usage,
                                        });
                                    }
                                }
                                mcp = Some(UsageWindow {
                                    label: "Total Monthly Web Search / Reader / Zread Quota"
                                        .to_string(),
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

                    // Emit windows in consistent order: 5h, weekly, MCP.
                    if let Some(w) = five_hour {
                        windows.push(w);
                    }
                    if let Some(w) = weekly {
                        windows.push(w);
                    }
                    if let Some(w) = mcp {
                        windows.push(w);
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
        if let Ok(data) = model_result
            && let Some(total) = data.total_usage
        {
            model_usage = Some(ModelUsageSnapshot {
                total_tokens: total.total_tokens_usage.unwrap_or(0),
                total_calls: total.total_model_call_count.unwrap_or(0),
                period_start_ms: start_ms,
                period_end_ms: now_ms,
                model_breakdown: vec![],
            });
        }

        // 3. Tool usage (24h window). Merge into MCP window's sub_items
        //    if the MCP window exists but has no sub_items.
        if let Ok(data) = tool_result
            && let Some(total) = data.total_usage
            && let Some(mcp) = windows
                .iter_mut()
                .find(|w| w.label == "Total Monthly Web Search / Reader / Zread Quota")
            && mcp.sub_items.is_empty()
        {
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

/// Z.ai wraps all responses in `{"code": 200, "msg": "...", "data": {...}, "success": true}`.
#[derive(Debug, Deserialize)]
struct ZaiEnvelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct QuotaLimitData {
    level: Option<String>,
    limits: Option<Vec<QuotaLimitItem>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaLimitItem {
    #[serde(rename = "type")]
    limit_type: String,
    /// Token limit window duration unit: 3 = hours, 6 = days.
    #[serde(default)]
    unit: Option<u64>,
    /// Window duration count (e.g. `unit=3, number=5` means 5 hours).
    #[serde(default)]
    number: Option<u64>,
    percentage: Option<f64>,
    /// Total token budget (only present for TOKENS_LIMIT in some plans).
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    current_value: Option<u64>,
    #[serde(default)]
    used: Option<u64>,
    /// For TIME_LIMIT: the maximum allowed count.
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
struct ModelUsageEnvelope {
    data: ModelUsageData,
}

#[derive(Debug, Deserialize)]
struct ModelUsageData {
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
struct ToolUsageEnvelope {
    data: ToolUsageData,
}

#[derive(Debug, Deserialize)]
struct ToolUsageData {
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
        // Real Z.ai response format: envelope with data.
        let json = r#"{
            "code": 200,
            "msg": "Operation successful",
            "data": {
                "level": "pro",
                "limits": [
                    {
                        "type": "TIME_LIMIT",
                        "unit": 5,
                        "number": 1,
                        "usage": 1000,
                        "currentValue": 123,
                        "percentage": 12.3,
                        "nextResetTime": 1746300000000,
                        "usageDetails": [
                            {"modelCode": "search-prime", "usage": 5678}
                        ]
                    },
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 3,
                        "number": 5,
                        "percentage": 40.5,
                        "nextResetTime": 1746600000000
                    },
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 6,
                        "number": 1,
                        "percentage": 13.0,
                        "nextResetTime": 1747000000000
                    }
                ]
            },
            "success": true
        }"#;
        let envelope: ZaiEnvelope<QuotaLimitData> = serde_json::from_str(json).unwrap();
        let data = envelope.data;
        assert_eq!(data.level.as_deref(), Some("pro"));
        let limits = data.limits.unwrap();
        assert_eq!(limits.len(), 3);
        assert_eq!(limits[0].limit_type, "TIME_LIMIT");
        assert_eq!(limits[0].current_value, Some(123));
        assert_eq!(limits[0].usage, Some(1000));
        assert_eq!(limits[1].limit_type, "TOKENS_LIMIT");
        assert_eq!(limits[1].unit, Some(3));
        assert_eq!(limits[1].number, Some(5));
        assert_eq!(limits[1].percentage, Some(40.5));
        assert_eq!(limits[2].limit_type, "TOKENS_LIMIT");
        assert_eq!(limits[2].unit, Some(6));
        assert_eq!(limits[2].number, Some(1));
    }

    #[test]
    fn deserialize_model_usage_response() {
        let json = r#"{
            "code": 200,
            "msg": "Operation successful",
            "data": {
                "totalUsage": {
                    "totalTokensUsage": 12500000,
                    "totalModelCallCount": 1234
                }
            },
            "success": true
        }"#;
        let envelope: ModelUsageEnvelope = serde_json::from_str(json).unwrap();
        let total = envelope.data.total_usage.unwrap();
        assert_eq!(total.total_tokens_usage, Some(12_500_000));
        assert_eq!(total.total_model_call_count, Some(1234));
    }

    #[test]
    fn deserialize_tool_usage_response() {
        let json = r#"{
            "code": 200,
            "msg": "Operation successful",
            "data": {
                "totalUsage": {
                    "totalNetworkSearchCount": 5678,
                    "totalWebReadMcpCount": 2345,
                    "totalZreadMcpCount": 890
                }
            },
            "success": true
        }"#;
        let envelope: ToolUsageEnvelope = serde_json::from_str(json).unwrap();
        let total = envelope.data.total_usage.unwrap();
        assert_eq!(total.total_network_search_count, 5678);
        assert_eq!(total.total_web_read_mcp_count, 2345);
        assert_eq!(total.total_zread_mcp_count, 890);
    }
}
