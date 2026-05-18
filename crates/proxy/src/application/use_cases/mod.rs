//! Use cases — orchestrators that own no framework deps.

pub mod admin;
pub mod handle_messages;

pub use admin::{
    CompleteAnthropicOAuth, CompleteOpenAiOAuth, GetAccountUsage, GetConfig, GetQuotaStatus,
    GetRecentRequests, GetStatus, GetUsageSummary, StartAnthropicOAuth, StartOpenAiOAuth,
    TestProvider, UpdateConfig,
};
pub use handle_messages::{CountTokensInput, CountTokensOutput, HandleMessages, HandleMessagesInput, HandleMessagesOutput};
