//! HandleMessages use case — orchestrates the proxy's /v1/messages flow.
//!
//! Depends only on ports. No axum, no reqwest, no rusqlite.

use crate::application::errors::ProxyError;
use crate::application::ports::{
    ApiFormat, BoxedByteStream, Provider, QuotaPort, RequestLogPort, UpstreamResponse, UsageParser,
};
use crate::domain::{RequestStart, RequestUsage, UsageRecord};
use bytes::Bytes;
use http::HeaderMap;
use shared::application::ports::{Clock, PricingRepository};
use shared::domain::value_objects::{ModelId, PricePerToken};
use std::sync::Arc;
use uuid::Uuid;

pub struct CountTokensInput {
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub struct CountTokensOutput {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
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
    quota: Arc<dyn QuotaPort>,
}

impl HandleMessages {
    pub fn new(
        provider: Arc<dyn Provider>,
        request_log: Arc<dyn RequestLogPort>,
        pricing: Arc<dyn PricingRepository>,
        clock: Arc<dyn Clock>,
        local_user_id: i64,
        quota: Arc<dyn QuotaPort>,
    ) -> Self {
        Self {
            provider,
            request_log,
            pricing,
            clock,
            local_user_id,
            quota,
        }
    }

    /// Forward a `/v1/messages/count_tokens` request to the upstream provider.
    /// This endpoint is required by Claude Code and other Anthropic clients
    /// for pre-flight token counting.
    ///
    /// If the upstream returns a non-2xx status (e.g. 404 when the provider
    /// doesn't support this endpoint), falls back to local token estimation
    /// from the request body.
    pub async fn count_tokens(
        &self,
        input: CountTokensInput,
    ) -> Result<CountTokensOutput, ProxyError> {
        let upstream = self
            .provider
            .forward(
                "/v1/messages/count_tokens",
                &input.headers,
                input.body.clone(),
                false,
            )
            .await;

        match upstream {
            Ok(UpstreamResponse::Buffered {
                status,
                headers,
                body,
                ..
            }) => {
                if (200..300).contains(&status) {
                    Ok(CountTokensOutput {
                        status,
                        headers,
                        body,
                    })
                } else {
                    // Upstream returned an error — fall back to local estimation.
                    tracing::debug!(
                        upstream_status = status,
                        "count_tokens upstream returned non-2xx; falling back to local estimation"
                    );
                    let estimated = estimate_tokens(&input.body);
                    let resp = serde_json::json!({ "input_tokens": estimated });
                    Ok(CountTokensOutput {
                        status: 200,
                        headers: HeaderMap::new(),
                        body: Bytes::from(serde_json::to_vec(&resp).unwrap_or_default()),
                    })
                }
            }
            Ok(UpstreamResponse::Streaming { .. }) => Err(ProxyError::BadRequest(
                "count_tokens returned unexpected streaming response".into(),
            )),
            Err(e) => {
                // Upstream error (transport, bad request, etc.) — fall back to local estimation.
                tracing::debug!(
                    error = %e,
                    "count_tokens upstream error; falling back to local estimation"
                );
                let estimated = estimate_tokens(&input.body);
                let resp = serde_json::json!({ "input_tokens": estimated });
                Ok(CountTokensOutput {
                    status: 200,
                    headers: HeaderMap::new(),
                    body: Bytes::from(serde_json::to_vec(&resp).unwrap_or_default()),
                })
            }
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
                // OpenAI providers only emit `usage` in the stream when the
                // request body sets `stream_options: {"include_usage": true}`.
                // Inject it transparently for streaming requests so logged
                // token counts aren't NULL.
                let outbound_body = if streaming {
                    ensure_include_usage(&input.body)
                } else {
                    input.body.clone()
                };
                self.provider
                    .forward_openai(
                        "/chat/completions",
                        &input.headers,
                        outbound_body,
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
                provider_id,
                translation_direction,
            } => self.handle_buffered(
                request_id,
                model,
                status,
                headers,
                body,
                input.api_format,
                provider_id,
                translation_direction,
            ),
            UpstreamResponse::Streaming {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } => Ok(self.handle_streaming(
                request_id,
                model,
                status,
                headers,
                body,
                input.api_format,
                provider_id,
                translation_direction,
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_buffered(
        &self,
        request_id: String,
        model: String,
        status: u16,
        headers: HeaderMap,
        body: Bytes,
        api_format: ApiFormat,
        provider_id: String,
        translation_direction: Option<String>,
    ) -> Result<HandleMessagesOutput, ProxyError> {
        if (200..300).contains(&status) {
            let usage = match api_format {
                ApiFormat::Anthropic => self.provider.parse_usage_json(&body).unwrap_or_default(),
                ApiFormat::OpenAI => self
                    .provider
                    .parse_usage_json_openai(&body)
                    .unwrap_or_default(),
            };
            let cost = compute_cost(&self.pricing, &model, &usage);
            let mut req_usage = to_request_usage(&usage, cost);
            req_usage.translation_direction = translation_direction;
            if let Err(e) = self
                .request_log
                .complete(&request_id, self.clock.now_ms(), &req_usage)
            {
                tracing::error!(request_id = %request_id, error = %e, "failed to record completed row");
            }
            self.quota.record(&provider_id, &req_usage);
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

    #[allow(clippy::too_many_arguments)]
    fn handle_streaming(
        &self,
        request_id: String,
        model: String,
        status: u16,
        headers: HeaderMap,
        body: BoxedByteStream,
        api_format: ApiFormat,
        provider_id: String,
        translation_direction: Option<String>,
    ) -> HandleMessagesOutput {
        let parser = match api_format {
            ApiFormat::Anthropic => self.provider.usage_parser(),
            ApiFormat::OpenAI => self.provider.usage_parser_openai(),
        };
        let pricing = self.pricing.clone();
        let request_log = self.request_log.clone();
        let clock = self.clock.clone();
        let quota = self.quota.clone();
        let req_id = request_id.clone();
        let model_clone = model.clone();
        let on_finish = Box::new(move |usage: UsageRecord, normal: bool| {
            let cost = compute_cost(&pricing, &model_clone, &usage);
            let mut req_usage = to_request_usage(&usage, cost);
            req_usage.translation_direction = translation_direction;
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
            quota.record(&provider_id, &req_usage);
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

/// Inject `stream_options: {"include_usage": true}` into an OpenAI-format
/// request body. OpenAI providers only emit token counts on the final SSE
/// chunk when this flag is set; without it, the proxy can't log usage for
/// streaming requests. Non-JSON or non-object bodies are returned verbatim.
fn ensure_include_usage(body: &Bytes) -> Bytes {
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return body.clone();
    };
    let Some(obj) = value.as_object_mut() else {
        return body.clone();
    };
    let stream_options = obj
        .entry("stream_options".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if let Some(so_obj) = stream_options.as_object_mut() {
        so_obj.insert("include_usage".to_string(), serde_json::json!(true));
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .unwrap_or_else(|_| body.clone())
}

fn to_request_usage(usage: &UsageRecord, cost_usd: Option<f64>) -> RequestUsage {
    RequestUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cost_usd,
        translation_direction: None, // caller sets this from UpstreamResponse
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

/// Estimate token count from an Anthropic-format request body when the
/// upstream provider doesn't support `/v1/messages/count_tokens`.
///
/// Extracts all string content from the `messages` array (including `system`
/// messages) and `system` field, then uses a heuristic of chars/4 for ASCII
/// and chars/2 for CJK characters. Returns at least 1.
fn estimate_tokens(body: &[u8]) -> u32 {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        tracing::warn!("count_tokens fallback: failed to parse request body as JSON");
        return 1;
    };

    let mut total_chars: usize = 0;
    let mut cjk_chars: usize = 0;

    fn count_text(s: &str, total: &mut usize, cjk: &mut usize) {
        for ch in s.chars() {
            *total += 1;
            // Rough CJK detection: Unicode ranges for Chinese, Japanese, Korean
            if matches!(ch,
                '\u{4E00}'..='\u{9FFF}' | // CJK Unified Ideographs
                '\u{3400}'..='\u{4DBF}' | // CJK Unified Ideographs Extension A
                '\u{AC00}'..='\u{D7AF}' | // Hangul Syllables
                '\u{3040}'..='\u{309F}' | // Hiragana
                '\u{30A0}'..='\u{30FF}'   // Katakana
            ) {
                *cjk += 1;
            }
        }
    }

    // Collect text from `messages` array
    if let Some(messages) = value.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            if let Some(content) = msg.get("content") {
                match content {
                    serde_json::Value::String(s) => count_text(s, &mut total_chars, &mut cjk_chars),
                    serde_json::Value::Array(parts) => {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                count_text(text, &mut total_chars, &mut cjk_chars);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Also check top-level `system` field
    if let Some(system) = value.get("system") {
        match system {
            serde_json::Value::String(s) => count_text(s, &mut total_chars, &mut cjk_chars),
            serde_json::Value::Array(parts) => {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        count_text(text, &mut total_chars, &mut cjk_chars);
                    }
                }
            }
            _ => {}
        }
    }

    let non_cjk = total_chars.saturating_sub(cjk_chars);
    // ASCII ~4 chars per token, CJK ~2 chars per token (very rough)
    let estimated = (non_cjk / 4) + (cjk_chars / 2);
    (estimated as u32).max(1)
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

    #[test]
    fn estimate_tokens_returns_at_least_1() {
        let body = br#"{}"#;
        assert!(estimate_tokens(body) >= 1);
    }

    #[test]
    fn estimate_tokens_counts_simple_message() {
        let body = br#"{"messages":[{"role":"user","content":"hello world"}]}"#;
        let estimated = estimate_tokens(body);
        // "hello world" = 11 chars, 11/4 = 2
        assert_eq!(estimated, 2);
    }

    #[test]
    fn estimate_tokens_counts_cjk_text() {
        let json = r#"{"messages":[{"role":"user","content":"\u4f60\u597d\u4e16\u754c"}]}"#;
        let body = Bytes::from(json);
        let estimated = estimate_tokens(&body);
        // "你好世界" = 4 CJK chars, 4/2 = 2
        assert_eq!(estimated, 2);
    }

    #[test]
    fn estimate_tokens_counts_system_prompt() {
        let body = br#"{"system":"you are helpful","messages":[{"role":"user","content":"hi"}]}"#;
        let estimated = estimate_tokens(body);
        // "you are helpful" = 16 chars, "hi" = 2 chars, total 18/4 = 4
        assert_eq!(estimated, 4);
    }

    #[test]
    fn estimate_tokens_counts_multipart_content() {
        let body = br#"{"messages":[{"role":"user","content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]}]}"#;
        let estimated = estimate_tokens(body);
        // "hello" + "world" = 10 chars, 10/4 = 2
        assert_eq!(estimated, 2);
    }

    #[test]
    fn estimate_tokens_handles_invalid_json() {
        let body = b"not json";
        assert_eq!(estimate_tokens(body), 1);
    }

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

    struct NoopQuota;
    impl crate::application::ports::QuotaPort for NoopQuota {
        fn check(&self, _: &str) -> crate::domain::quota::QuotaCheck {
            crate::domain::quota::QuotaCheck::Ok
        }
        fn record(&self, _: &str, _: &crate::domain::RequestUsage) {}
        fn snapshot(&self) -> Vec<crate::domain::quota::QuotaSnapshot> {
            vec![]
        }
    }

    fn noop_quota() -> Arc<dyn crate::application::ports::QuotaPort> {
        Arc::new(NoopQuota)
    }

    #[tokio::test]
    async fn invalid_model_returns_bad_request_without_inserting_row() {
        let provider: Arc<dyn Provider> =
            Arc::new(FakeProvider::new(Err("missing 'model'".into())));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(
            provider,
            log_dyn,
            fake_pricing(),
            fake_clock(),
            1,
            noop_quota(),
        );

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
            provider_id: "fake".into(),
            translation_direction: None,
        })));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(
            Arc::new(prov),
            log_dyn,
            fake_pricing(),
            fake_clock(),
            1,
            noop_quota(),
        );

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
            provider_id: "fake".into(),
            translation_direction: None,
        })));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(
            Arc::new(prov),
            log_dyn,
            fake_pricing(),
            fake_clock(),
            1,
            noop_quota(),
        );

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
            provider_id: "fake".into(),
            translation_direction: None,
        })));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(
            Arc::new(prov),
            log_dyn,
            fake_pricing(),
            fake_clock(),
            1,
            noop_quota(),
        );

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

    fn parse_json(b: &Bytes) -> serde_json::Value {
        serde_json::from_slice(b).expect("ensure_include_usage must emit valid json")
    }

    #[test]
    fn ensure_include_usage_adds_field_when_missing() {
        let body = Bytes::from_static(br#"{"model":"glm-4.6","stream":true}"#);
        let out = ensure_include_usage(&body);
        let v = parse_json(&out);
        assert_eq!(
            v["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
        assert_eq!(v["model"], serde_json::json!("glm-4.6"));
        assert_eq!(v["stream"], serde_json::json!(true));
    }

    #[test]
    fn ensure_include_usage_fills_empty_stream_options() {
        let body = Bytes::from_static(br#"{"stream":true,"stream_options":{}}"#);
        let out = ensure_include_usage(&body);
        let v = parse_json(&out);
        assert_eq!(
            v["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn ensure_include_usage_flips_existing_false_value() {
        let body =
            Bytes::from_static(br#"{"stream":true,"stream_options":{"include_usage":false}}"#);
        let out = ensure_include_usage(&body);
        let v = parse_json(&out);
        assert_eq!(
            v["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn ensure_include_usage_preserves_other_stream_options_keys() {
        let body = Bytes::from_static(
            br#"{"stream":true,"stream_options":{"include_usage":true,"continuous_usage_stats":true}}"#,
        );
        let out = ensure_include_usage(&body);
        let v = parse_json(&out);
        assert_eq!(
            v["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
        assert_eq!(
            v["stream_options"]["continuous_usage_stats"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn ensure_include_usage_returns_non_json_body_unchanged() {
        let body = Bytes::from_static(b"not json at all");
        let out = ensure_include_usage(&body);
        assert_eq!(out, body);
    }

    #[test]
    fn ensure_include_usage_returns_non_object_json_unchanged() {
        let body = Bytes::from_static(br#"["this","is","an","array"]"#);
        let out = ensure_include_usage(&body);
        assert_eq!(out, body);
    }

    #[tokio::test]
    async fn forward_transport_error_propagates_as_proxy_error() {
        let mut prov = FakeProvider::new(Ok("model-x".into()));
        prov.forward_response = Mutex::new(Some(Err(ProxyError::BadRequest("simulated".into()))));
        let log = Arc::new(FakeRequestLog::default());
        let log_dyn: Arc<dyn RequestLogPort> = log.clone();
        let uc = HandleMessages::new(
            Arc::new(prov),
            log_dyn,
            fake_pricing(),
            fake_clock(),
            1,
            noop_quota(),
        );

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
