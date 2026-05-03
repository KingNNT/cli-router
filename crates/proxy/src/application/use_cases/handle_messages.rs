//! HandleMessages use case — orchestrates the proxy's /v1/messages flow.
//!
//! Depends only on ports. No axum, no reqwest, no rusqlite.

use crate::application::errors::ProxyError;
use crate::application::ports::{
    BoxedByteStream, Provider, RequestLogPort, UpstreamResponse, UsageParser,
};
use crate::domain::{RequestStart, RequestUsage, UsageRecord};
use bytes::Bytes;
use http::HeaderMap;
use shared::application::ports::{Clock, PricingRepository};
use shared::domain::value_objects::{ModelId, PricePerToken};
use std::sync::Arc;
use uuid::Uuid;

/// Which API format the client used — determines which `forward_*` method
/// the use case calls on the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    Anthropic,
    OpenAI,
}

pub struct HandleMessagesInput {
    pub headers: HeaderMap,
    pub body: Bytes,
    pub api_format: ApiFormat,
}

pub enum HandleMessagesOutput {
    Buffered {
        status: u16,
        headers: HeaderMap,
        body: Bytes,
    },
    Streaming {
        status: u16,
        headers: HeaderMap,
        body: BoxedByteStream,
        usage_parser: Box<dyn UsageParser>,
        on_finish: Box<dyn FnOnce(UsageRecord, bool) + Send>,
    },
}

pub struct HandleMessages {
    provider: Arc<dyn Provider>,
    request_log: Arc<dyn RequestLogPort>,
    pricing: Arc<dyn PricingRepository>,
    clock: Arc<dyn Clock>,
    local_user_id: i64,
}

impl HandleMessages {
    pub fn new(
        provider: Arc<dyn Provider>,
        request_log: Arc<dyn RequestLogPort>,
        pricing: Arc<dyn PricingRepository>,
        clock: Arc<dyn Clock>,
        local_user_id: i64,
    ) -> Self {
        Self {
            provider,
            request_log,
            pricing,
            clock,
            local_user_id,
        }
    }

    pub async fn execute(
        &self,
        input: HandleMessagesInput,
    ) -> Result<HandleMessagesOutput, ProxyError> {
        let model = self
            .provider
            .parse_model(&input.body)
            .map_err(ProxyError::BadRequest)?;
        let request_id = Uuid::new_v4().to_string();
        let started_at = self.clock.now_ms();

        self.request_log.insert_started(&RequestStart {
            id: request_id.clone(),
            user_id: self.local_user_id,
            provider: self.provider.name().to_string(),
            model: model.clone(),
            started_at,
        })?;

        let streaming = is_streaming(&input.body);
        let upstream = match input.api_format {
            ApiFormat::Anthropic => {
                self.provider
                    .forward(
                        "/v1/messages",
                        &input.headers,
                        input.body.clone(),
                        streaming,
                    )
                    .await?
            }
            ApiFormat::OpenAI => {
                self.provider
                    .forward_openai(
                        "/v1/chat/completions",
                        &input.headers,
                        input.body.clone(),
                        streaming,
                    )
                    .await?
            }
        };

        match upstream {
            UpstreamResponse::Buffered {
                status,
                headers,
                body,
            } => self.handle_buffered(request_id, model, status, headers, body),
            UpstreamResponse::Streaming {
                status,
                headers,
                body,
            } => Ok(self.handle_streaming(request_id, model, status, headers, body)),
        }
    }

    fn handle_buffered(
        &self,
        request_id: String,
        model: String,
        status: u16,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<HandleMessagesOutput, ProxyError> {
        if (200..300).contains(&status) {
            let usage = self.provider.parse_usage_json(&body).unwrap_or_default();
            let cost = compute_cost(&self.pricing, &model, &usage);
            let req_usage = to_request_usage(&usage, cost);
            if let Err(e) = self
                .request_log
                .complete(&request_id, self.clock.now_ms(), &req_usage)
            {
                tracing::error!(request_id = %request_id, error = %e, "failed to record completed row");
            }
            Ok(HandleMessagesOutput::Buffered {
                status,
                headers,
                body,
            })
        } else {
            let msg = String::from_utf8_lossy(&body).to_string();
            if let Err(e) = self.request_log.fail(
                &request_id,
                self.clock.now_ms(),
                &msg,
                &RequestUsage::default(),
            ) {
                tracing::error!(request_id = %request_id, error = %e, "failed to record errored row");
            }
            Ok(HandleMessagesOutput::Buffered {
                status,
                headers,
                body,
            })
        }
    }

    fn handle_streaming(
        &self,
        request_id: String,
        model: String,
        status: u16,
        headers: HeaderMap,
        body: BoxedByteStream,
    ) -> HandleMessagesOutput {
        let parser = self.provider.usage_parser();
        let pricing = self.pricing.clone();
        let request_log = self.request_log.clone();
        let clock = self.clock.clone();
        let req_id = request_id.clone();
        let model_clone = model.clone();
        let on_finish = Box::new(move |usage: UsageRecord, normal: bool| {
            let cost = compute_cost(&pricing, &model_clone, &usage);
            let req_usage = to_request_usage(&usage, cost);
            let result = if normal {
                request_log.complete(&req_id, clock.now_ms(), &req_usage)
            } else {
                request_log.fail(&req_id, clock.now_ms(), "client disconnected", &req_usage)
            };
            if let Err(e) = result {
                tracing::error!(
                    request_id = %req_id,
                    error = %e,
                    "failed to record streaming completion"
                );
            }
        });
        HandleMessagesOutput::Streaming {
            status,
            headers,
            body,
            usage_parser: parser,
            on_finish,
        }
    }
}

fn is_streaming(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false)
}

fn to_request_usage(usage: &UsageRecord, cost_usd: Option<f64>) -> RequestUsage {
    RequestUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cost_usd,
    }
}

fn compute_cost(
    pricing: &Arc<dyn PricingRepository>,
    model: &str,
    usage: &UsageRecord,
) -> Option<f64> {
    let model_id = match ModelId::new(model.to_string()) {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(model = %model, error = %e, "invalid model id; cost will be NULL");
            return None;
        }
    };
    let keys = model_id.lookup_keys();
    let map = match pricing.find_many(&keys) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "pricing lookup failed; cost will be NULL");
            return None;
        }
    };
    let p = match keys.iter().find_map(|k| map.get(k)) {
        Some(p) => p,
        None => {
            tracing::warn!(model = %model, "no pricing entry; cost will be NULL");
            return None;
        }
    };
    let i = usage.input_tokens.unwrap_or(0) as f64;
    let o = usage.output_tokens.unwrap_or(0) as f64;
    let cr = usage.cache_read_tokens.unwrap_or(0) as f64;
    let cw = usage.cache_creation_tokens.unwrap_or(0) as f64;
    let cost = i * p.input_rate.value()
        + o * p.output_rate.value()
        + cr * p
            .cache_read_rate
            .as_ref()
            .map(PricePerToken::value)
            .unwrap_or(0.0)
        + cw * p
            .cache_write_rate
            .as_ref()
            .map(PricePerToken::value)
            .unwrap_or(0.0);
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{BoxedByteStream, BoxedError, UpstreamResponse};
    use async_trait::async_trait;
    use bytes::Bytes;
    use http::HeaderMap;
    use shared::application::test_support::{FakePricingRepository, FixedClock};
    use std::sync::Mutex;

    /// Fake provider that returns prearranged responses.
    struct FakeProvider {
        forward_response: Mutex<Option<Result<UpstreamResponse, ProxyError>>>,
        parse_model_response: Mutex<Result<String, String>>,
        parse_usage_json_response: UsageRecord,
    }

    impl FakeProvider {
        fn new(parse_model_response: Result<String, String>) -> Self {
            Self {
                forward_response: Mutex::new(None),
                parse_model_response: Mutex::new(parse_model_response),
                parse_usage_json_response: UsageRecord::default(),
            }
        }
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn parse_model(&self, _body: &[u8]) -> Result<String, String> {
            self.parse_model_response.lock().unwrap().clone()
        }
        fn usage_parser(&self) -> Box<dyn UsageParser> {
            struct StubParser;
            impl UsageParser for StubParser {
                fn feed(&mut self, _chunk: &[u8]) {}
                fn finish(self: Box<Self>) -> UsageRecord {
                    UsageRecord::default()
                }
            }
            Box::new(StubParser)
        }
        fn parse_usage_json(&self, _body: &[u8]) -> Result<UsageRecord, String> {
            Ok(self.parse_usage_json_response.clone())
        }
        async fn forward(
            &self,
            _path: &str,
            _headers: &HeaderMap,
            _body: Bytes,
            _streaming: bool,
        ) -> Result<UpstreamResponse, ProxyError> {
            self.forward_response
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Err(ProxyError::BadRequest("no fake response prepared".into())))
        }
    }

    /// Fake request log that records calls.
    #[derive(Default)]
    struct FakeRequestLog {
        started: Mutex<Vec<RequestStart>>,
        completed: Mutex<Vec<(String, i64, RequestUsage)>>,
        failed: Mutex<Vec<(String, i64, String, RequestUsage)>>,
    }
    impl RequestLogPort for FakeRequestLog {
        fn insert_started(&self, start: &RequestStart) -> Result<(), ProxyError> {
            self.started.lock().unwrap().push(start.clone());
            Ok(())
        }
        fn complete(
            &self,
            id: &str,
            finished_at: i64,
            usage: &RequestUsage,
        ) -> Result<(), ProxyError> {
            self.completed
                .lock()
                .unwrap()
                .push((id.to_string(), finished_at, usage.clone()));
            Ok(())
        }
        fn fail(
            &self,
            id: &str,
            finished_at: i64,
            msg: &str,
            usage: &RequestUsage,
        ) -> Result<(), ProxyError> {
            self.failed.lock().unwrap().push((
                id.to_string(),
                finished_at,
                msg.to_string(),
                usage.clone(),
            ));
            Ok(())
        }
    }

    fn fake_clock() -> Arc<dyn Clock> {
        Arc::new(FixedClock::new(2026, 5, 2))
    }

    fn fake_pricing() -> Arc<dyn PricingRepository> {
        Arc::new(FakePricingRepository::default())
    }

    #[tokio::test]
    async fn invalid_model_returns_bad_request_without_inserting_row() {
        let provider: Arc<dyn Provider> =
            Arc::new(FakeProvider::new(Err("missing 'model'".into())));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(provider, log_dyn, fake_pricing(), fake_clock(), 1);

        let result = uc
            .execute(HandleMessagesInput {
                headers: HeaderMap::new(),
                body: Bytes::from_static(b"{}"),
                api_format: ApiFormat::Anthropic,
            })
            .await;

        assert!(matches!(result, Err(ProxyError::BadRequest(_))));
        assert!(
            log.started.lock().unwrap().is_empty(),
            "row must not be inserted"
        );
    }

    #[tokio::test]
    async fn buffered_2xx_records_completed_row() {
        let mut prov = FakeProvider::new(Ok("claude-3-5-sonnet-20241022".into()));
        prov.forward_response = Mutex::new(Some(Ok(UpstreamResponse::Buffered {
            status: 200,
            headers: HeaderMap::new(),
            body: Bytes::from_static(br#"{"usage":{"input_tokens":10,"output_tokens":20}}"#),
        })));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(Arc::new(prov), log_dyn, fake_pricing(), fake_clock(), 1);

        let output = uc
            .execute(HandleMessagesInput {
                headers: HeaderMap::new(),
                body: Bytes::from_static(
                    br#"{"model":"claude-3-5-sonnet-20241022","stream":false}"#,
                ),
                api_format: ApiFormat::Anthropic,
            })
            .await
            .unwrap();

        assert!(matches!(
            output,
            HandleMessagesOutput::Buffered { status: 200, .. }
        ));
        assert_eq!(log.started.lock().unwrap().len(), 1);
        assert_eq!(log.completed.lock().unwrap().len(), 1);
        assert!(log.failed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn buffered_non2xx_records_errored_row_and_forwards_status() {
        let mut prov = FakeProvider::new(Ok("model-x".into()));
        prov.forward_response = Mutex::new(Some(Ok(UpstreamResponse::Buffered {
            status: 401,
            headers: HeaderMap::new(),
            body: Bytes::from_static(br#"{"error":"unauthorized"}"#),
        })));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(Arc::new(prov), log_dyn, fake_pricing(), fake_clock(), 1);

        let output = uc
            .execute(HandleMessagesInput {
                headers: HeaderMap::new(),
                body: Bytes::from_static(br#"{"model":"model-x"}"#),
                api_format: ApiFormat::Anthropic,
            })
            .await
            .unwrap();

        assert!(matches!(
            output,
            HandleMessagesOutput::Buffered { status: 401, .. }
        ));
        assert_eq!(log.failed.lock().unwrap().len(), 1);
        assert!(log.completed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn streaming_returns_streaming_output() {
        use futures::stream;
        let mut prov = FakeProvider::new(Ok("model-x".into()));
        let chunks: Vec<Result<Bytes, BoxedError>> =
            vec![Ok(Bytes::from_static(b"event: ping\ndata: {}\n\n"))];
        let body: BoxedByteStream = Box::pin(stream::iter(chunks));
        prov.forward_response = Mutex::new(Some(Ok(UpstreamResponse::Streaming {
            status: 200,
            headers: HeaderMap::new(),
            body,
        })));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(Arc::new(prov), log_dyn, fake_pricing(), fake_clock(), 1);

        let output = uc
            .execute(HandleMessagesInput {
                headers: HeaderMap::new(),
                body: Bytes::from_static(br#"{"model":"model-x"}"#),
                api_format: ApiFormat::Anthropic,
            })
            .await
            .unwrap();

        assert!(matches!(
            output,
            HandleMessagesOutput::Streaming { status: 200, .. }
        ));
        // Don't drive the stream here — just confirm shape. Drop tests are in stream.rs.
    }

    #[tokio::test]
    async fn forward_transport_error_propagates_as_proxy_error() {
        let mut prov = FakeProvider::new(Ok("model-x".into()));
        prov.forward_response = Mutex::new(Some(Err(ProxyError::BadRequest("simulated".into()))));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(Arc::new(prov), log_dyn, fake_pricing(), fake_clock(), 1);

        let result = uc
            .execute(HandleMessagesInput {
                headers: HeaderMap::new(),
                body: Bytes::from_static(br#"{"model":"model-x"}"#),
                api_format: ApiFormat::Anthropic,
            })
            .await;

        assert!(matches!(result, Err(ProxyError::BadRequest(_))));
        // Started row WAS inserted before forward was attempted; that's correct behavior.
        assert_eq!(log.started.lock().unwrap().len(), 1);
        assert!(log.completed.lock().unwrap().is_empty());
        assert!(
            log.failed.lock().unwrap().is_empty(),
            "no fail row when forward errors before response"
        );
    }
}
