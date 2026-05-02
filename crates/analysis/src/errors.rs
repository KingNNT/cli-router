use shared::application::errors::ApplicationError;

#[derive(thiserror::Error, Debug)]
pub enum FrameworkError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("terminal error: {0}")]
    Terminal(String),

    #[error(transparent)]
    Application(#[from] ApplicationError),
}
