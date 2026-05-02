//! Proxy application: use cases, ports, error type. Ring 1.
//! Imports: domain, std/serde/chrono, the workspace `application` crate's ports
//! (Clock, PricingRepository).

pub mod errors;
pub mod ports;
pub mod use_cases;

pub use errors::ProxyError;
