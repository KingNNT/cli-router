//! Adapter layer — shared gateways and clock. Depends on application and domain.

pub mod clock;
pub mod errors;
pub mod gateways;

pub use errors::AdapterError;
