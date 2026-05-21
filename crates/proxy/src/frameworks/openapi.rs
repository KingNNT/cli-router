//! OpenAPI spec generation for the CLI Router Proxy.

use axum::Router;
use utoipa::OpenApi;

// ── Typed schemas for proxy endpoints (documentation-only) ──────────

/// Request body for `POST /v1/messages` (Anthropic Messages API).
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct AnthropicMessagesRequest {
    /// Model identifier (e.g. "claude-sonnet-4-20250514", "glm-5").
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<AnthropicMessage>,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Enable server-sent events streaming.
    #[serde(default)]
    pub stream: Option<bool>,
    /// System prompt.
    #[serde(default)]
    pub system: Option<String>,
    /// Sampling temperature (0.0–1.0).
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Top-p nucleus sampling.
    #[serde(default)]
    pub top_p: Option<f64>,
    /// Stop sequences.
    #[serde(default)]
    pub stop_sequences: Option<Vec<String>>,
}

/// A single message in an Anthropic conversation.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct AnthropicMessage {
    /// Role: "user" or "assistant".
    pub role: String,
    /// Message content (string or array of content blocks).
    pub content: serde_json::Value,
}

/// Request body for `POST /v1/chat/completions` (OpenAI Chat API).
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct OpenAIChatRequest {
    /// Model identifier. Use `provider/model` namespace syntax to override routing.
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<OpenAIChatMessage>,
    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Enable server-sent events streaming.
    #[serde(default)]
    pub stream: Option<bool>,
    /// Sampling temperature (0.0–2.0).
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Top-p nucleus sampling.
    #[serde(default)]
    pub top_p: Option<f64>,
}

/// A single message in an OpenAI chat conversation.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct OpenAIChatMessage {
    /// Role: "system", "user", "assistant", or "tool".
    pub role: String,
    /// Message content.
    pub content: serde_json::Value,
}

/// Request body for `POST /v1/messages/count_tokens`.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct CountTokensRequest {
    /// Model identifier.
    pub model: String,
    /// Messages to count tokens for.
    pub messages: Vec<AnthropicMessage>,
    /// System prompt (included in count).
    #[serde(default)]
    pub system: Option<String>,
}

// ── OpenAPI spec ─────────────────────────────────────────────────────

/// CLI Router Proxy API — all endpoints.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "CLI Router Proxy API",
        version = "1.0.0",
        description = "HTTP proxy for LLM providers with usage capture, routing, and admin dashboard."
    ),
    tags(
        (name = "Proxy", description = "LLM proxy endpoints"),
        (name = "Admin", description = "Configuration and status"),
        (name = "Usage", description = "Usage metrics and quotas"),
        (name = "Providers", description = "Provider management"),
        (name = "OAuth", description = "OAuth authentication flows"),
    ),
    paths(
        crate::frameworks::handler::messages,
        crate::frameworks::handler::count_tokens,
        crate::frameworks::handler::chat_completions,
        crate::frameworks::admin::status_handler,
        crate::frameworks::admin::get_config_handler,
        crate::frameworks::admin::update_config_handler,
        crate::frameworks::admin::recent_handler,
        crate::frameworks::admin::usage_summary_handler,
        crate::frameworks::admin::account_usage_handler,
        crate::frameworks::admin::quota_status_handler,
        crate::frameworks::admin::test_provider_handler,
        crate::frameworks::admin::oauth_start_handler,
        crate::frameworks::admin::oauth_complete_handler,
        crate::frameworks::admin::openai_oauth_start_handler,
        crate::frameworks::admin::openai_oauth_complete_handler,
    ),
    components(
        schemas(
            // Proxy endpoint schemas
            AnthropicMessagesRequest,
            AnthropicMessage,
            OpenAIChatRequest,
            OpenAIChatMessage,
            CountTokensRequest,
            // Admin API schemas — all from proxy-admin-api
            proxy_admin_api::StatusResponse,
            proxy_admin_api::ConfigPayload,
            proxy_admin_api::ProviderPayload,
            proxy_admin_api::AuthPayload,
            proxy_admin_api::RoutingRulePayload,
            proxy_admin_api::RoutingStrategyPayload,
            proxy_admin_api::MatchPayload,
            proxy_admin_api::AffinityPayload,
            proxy_admin_api::QuotaPayload,
            proxy_admin_api::RecentRequestsResponse,
            proxy_admin_api::RecentRequestItem,
            proxy_admin_api::TestProviderRequest,
            proxy_admin_api::TestProviderResponse,
            proxy_admin_api::StartOAuthRequest,
            proxy_admin_api::StartOAuthResponse,
            proxy_admin_api::CompleteOAuthRequest,
            proxy_admin_api::CompleteOAuthResponse,
            proxy_admin_api::UsageSummaryResponse,
            proxy_admin_api::DailyUsageRow,
            proxy_admin_api::ModelUsageRow,
            proxy_admin_api::QuotaStatusListDto,
            proxy_admin_api::QuotaStatusDto,
            proxy_admin_api::QuotaMetricDto,
            proxy_admin_api::QuotaMetricState,
            proxy_admin_api::AccountUsageResponse,
            proxy_admin_api::ProviderAccountUsageDto,
            proxy_admin_api::ProviderUsageStatus,
            proxy_admin_api::UsageWindowDto,
            proxy_admin_api::UsageSubItemDto,
            proxy_admin_api::ModelBreakdownItemDto,
            proxy_admin_api::ModelUsageDto,
            proxy_admin_api::ApiError,
        )
    ),
)]
pub struct ApiDoc;

pub fn build_openapi_spec() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

pub fn build_docs_app() -> Router {
    use utoipa_redoc::Servable as _;
    use utoipa_swagger_ui::SwaggerUi;

    let spec = build_openapi_spec();

    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", spec.clone()))
        .merge(utoipa_redoc::Redoc::with_url("/redoc", spec))
}
