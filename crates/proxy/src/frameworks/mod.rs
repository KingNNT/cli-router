//! Frameworks ring: axum-specific glue + drivers.

pub mod admin;
pub mod error;
pub mod handler;
pub mod server;
pub mod stream;

pub use admin::{build_admin_router, AdminState};
pub use error::ProxyError;
pub use server::build_router;
