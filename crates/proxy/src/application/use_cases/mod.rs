//! Use cases — orchestrators that own no framework deps.

pub mod admin;
pub mod handle_messages;

pub use admin::{
    CompleteAnthropicOAuth, GetConfig, GetRecentRequests, GetStatus, StartAnthropicOAuth,
    TestProvider, UpdateConfig,
};
pub use handle_messages::{ApiFormat, HandleMessages, HandleMessagesInput, HandleMessagesOutput};
