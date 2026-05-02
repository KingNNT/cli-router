//! Domain layer — entities, value objects, pure services. Depends only on std and chrono.

pub mod errors;
pub use errors::DomainError;

pub mod value_objects;

pub mod entities;

pub mod services;
