//! Frameworks ring: axum-specific glue + drivers.

pub mod admin;
pub mod error;
pub mod handler;
pub mod server;
pub mod stream;

pub use admin::{AdminState, build_admin_router};
pub use error::ProxyError;
pub use server::build_router;
