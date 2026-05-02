//! Application layer — shared ports and errors. Depends only on domain.

pub mod errors;
pub mod ports;
pub mod test_support;

pub use errors::ApplicationError;
