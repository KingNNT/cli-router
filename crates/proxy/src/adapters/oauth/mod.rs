//! OAuth adapters.

pub mod anthropic;
pub mod openai;

pub use anthropic::{
    OAuthError, OAuthSessionStore, OAuthTokens, PkceCodes, build_authorize_url, exchange_code,
    generate_pkce, refresh_token,
};
// OpenAI OAuth types stay namespaced — access via oauth::openai::*
