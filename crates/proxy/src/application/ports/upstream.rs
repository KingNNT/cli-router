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
    },
    Streaming {
        status: u16,
        headers: HeaderMap,
        body: BoxedByteStream,
    },
}
