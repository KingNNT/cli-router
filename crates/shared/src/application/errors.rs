use crate::domain::DomainError;

#[derive(thiserror::Error, Debug)]
pub enum ApplicationError {
    #[error("repository error: {0}")]
    Repository(Box<dyn std::error::Error + Send + Sync>),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error(transparent)]
    Domain(#[from] DomainError),
}
