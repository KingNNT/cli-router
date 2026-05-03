//! Upstream HTTP response value types.

use bytes::Bytes;
use futures::Stream;
use http::HeaderMap;
use std::pin::Pin;

pub type BoxedError = Box<dyn std::error::Error + Send + Sync>;

pub type BoxedByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, BoxedError>> + Send>>;

pub enum UpstreamResponse {
    Buffered {
        status: u16,
        headers: HeaderMap,
        body: Bytes,
        /// The leaf provider config name that served this request (e.g. `"zai"`
        /// or `"anthropic"`). Populated by `RoutingProvider`; leaf providers
        /// populate it with their own `name()`.
        provider_id: String,
        /// e.g. `"anthropic→openai"` or `"openai→anthropic"`; `None` for passthrough.
        /// Populated by `RoutingProvider` after a successful translation; leaf
        /// providers always set this to `None`.
        translation_direction: Option<String>,
    },
    Streaming {
        status: u16,
        headers: HeaderMap,
        body: BoxedByteStream,
        /// See `Buffered::provider_id`.
        provider_id: String,
        /// See `Buffered::translation_direction`.
        translation_direction: Option<String>,
    },
}
