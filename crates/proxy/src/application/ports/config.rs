//! Port trait for config persistence — abstracts DB vs file storage.

use crate::config::ConfigError;

/// Read/write the full application configuration.
///
/// Clean architecture port — use cases depend on this trait, not on any
/// specific storage mechanism.
pub trait ConfigRepository: Send + Sync {
    /// Load the full config from the persistent store.
    fn load(&self) -> Result<crate::config::Config, ConfigError>;

    /// Save the full config to the persistent store.
    fn save(&self, config: &crate::config::Config) -> Result<(), ConfigError>;
}
