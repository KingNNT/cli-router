//! Use cases — orchestrators that own no framework deps.

pub mod admin;
pub mod handle_messages;

pub use admin::{
    CompleteAnthropicOAuth, GetConfig, GetQuotaStatus, GetRecentRequests, GetStatus,
    GetUsageSummary, StartAnthropicOAuth, TestProvider, UpdateConfig,
};
pub use handle_messages::{HandleMessages, HandleMessagesInput, HandleMessagesOutput};
