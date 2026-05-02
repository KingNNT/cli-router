use crate::domain::DomainError;

#[derive(thiserror::Error, Debug)]
pub enum AdapterError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("http error: {0}")]
    Http(#[from] Box<ureq::Error>),

    #[error("data mapping error: {0}")]
    DataMapping(String),

    #[error(transparent)]
    InvalidDomainValue(#[from] DomainError),
}

use crate::application::errors::ApplicationError;

impl From<AdapterError> for ApplicationError {
    fn from(e: AdapterError) -> Self {
        ApplicationError::Repository(Box::new(e))
    }
}
