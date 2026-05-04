//! Blocking ureq client for the proxy admin API.
//!
//! All calls go to `127.0.0.1:<port>` and return the wire types from
//! `proxy_admin_api`. Errors are surfaced as `ClientError` so the TUI can
//! show "daemon offline" without crashing.

use proxy_admin_api::{
    CompleteOAuthRequest, CompleteOAuthResponse, ConfigPayload, QuotaStatusListDto,
    RecentRequestsResponse, StartOAuthRequest, StartOAuthResponse, StatusResponse,
    TestProviderRequest, TestProviderResponse, UsageSummaryResponse,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("upstream {0}: {1}")]
    Status(u16, String),
}

pub struct AdminClient {
    base_url: String,
}

impl AdminClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    pub fn get_status(&self) -> Result<StatusResponse, ClientError> {
        get_json(&format!("{}/admin/status", self.base_url))
    }

    pub fn get_config(&self) -> Result<ConfigPayload, ClientError> {
        get_json(&format!("{}/admin/config", self.base_url))
    }

    pub fn put_config(&self, payload: &ConfigPayload) -> Result<ConfigPayload, ClientError> {
        let url = format!("{}/admin/config", self.base_url);
        let resp = ureq::put(&url)
            .set("content-type", "application/json")
            .send_json(payload)
            .map_err(map_ureq_err)?;
        resp.into_json::<ConfigPayload>()
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    pub fn get_recent(&self, limit: u32) -> Result<RecentRequestsResponse, ClientError> {
        get_json(&format!(
            "{}/admin/requests/recent?limit={limit}",
            self.base_url
        ))
    }

    pub fn get_usage_summary(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<UsageSummaryResponse, ClientError> {
        get_json(&format!(
            "{}/admin/usage/summary?from={from_ms}&to={to_ms}",
            self.base_url
        ))
    }

    #[allow(dead_code)]
    pub fn oauth_start(&self, provider_name: &str) -> Result<StartOAuthResponse, ClientError> {
        let url = format!("{}/admin/oauth/anthropic/start", self.base_url);
        let body = StartOAuthRequest {
            provider_name: provider_name.into(),
        };
        let resp = ureq::post(&url)
            .set("content-type", "application/json")
            .send_json(&body)
            .map_err(map_ureq_err)?;
        resp.into_json::<StartOAuthResponse>()
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    #[allow(dead_code)]
    pub fn oauth_complete(
        &self,
        state_id: &str,
        code: &str,
        provider_name: &str,
    ) -> Result<CompleteOAuthResponse, ClientError> {
        let url = format!("{}/admin/oauth/anthropic/complete", self.base_url);
        let body = CompleteOAuthRequest {
            state_id: state_id.into(),
            code: code.into(),
            provider_name: provider_name.into(),
        };
        let resp = ureq::post(&url)
            .set("content-type", "application/json")
            .send_json(&body)
            .map_err(map_ureq_err)?;
        resp.into_json::<CompleteOAuthResponse>()
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    pub fn get_quota_status(&self) -> Result<QuotaStatusListDto, ClientError> {
        get_json(&format!("{}/admin/quota/status", self.base_url))
    }

    pub fn test_provider(
        &self,
        name: &str,
        model: &str,
    ) -> Result<TestProviderResponse, ClientError> {
        let url = format!("{}/admin/providers/{name}/test", self.base_url);
        let body = TestProviderRequest {
            model: model.into(),
        };
        let resp = ureq::post(&url)
            .set("content-type", "application/json")
            .send_json(&body)
            .map_err(map_ureq_err)?;
        resp.into_json::<TestProviderResponse>()
            .map_err(|e| ClientError::Decode(e.to_string()))
    }
}

fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, ClientError> {
    let resp = ureq::get(url).call().map_err(map_ureq_err)?;
    resp.into_json::<T>()
        .map_err(|e| ClientError::Decode(e.to_string()))
}

fn map_ureq_err(e: ureq::Error) -> ClientError {
    match e {
        ureq::Error::Status(code, resp) => {
            let msg = resp
                .into_string()
                .unwrap_or_else(|_| format!("HTTP {code}"));
            ClientError::Status(code, msg)
        }
        ureq::Error::Transport(t) => ClientError::Transport(t.to_string()),
    }
}

#[cfg(test)]
mod usage_summary_client_tests {
    use super::*;
    use proxy_admin_api::{DailyUsageRow, ModelUsageRow, UsageSummaryResponse};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn get_usage_summary_calls_endpoint_and_decodes_response() {
        let server = MockServer::start().await;
        let payload = UsageSummaryResponse {
            from_ms: 100,
            to_ms: 200,
            daily: vec![DailyUsageRow {
                date: "2026-05-03".into(),
                requests: 1,
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: 0.01,
            }],
            models: vec![ModelUsageRow {
                model: "m".into(),
                provider: "anthropic".into(),
                requests: 1,
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: 0.01,
            }],
        };

        Mock::given(method("GET"))
            .and(path("/admin/usage/summary"))
            .and(query_param("from", "100"))
            .and(query_param("to", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = AdminClient::new(server.uri());
        let got = client.get_usage_summary(100, 200).unwrap();
        assert_eq!(got, payload);
    }
}
