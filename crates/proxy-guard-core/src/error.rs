use thiserror::Error;

#[derive(Debug, Error)]
pub enum GuardError {
    #[error("CONFIG_INVALID: {0}")]
    Config(String),
    #[error("CONFIG_IO: {0}")]
    Io(String),
    #[error("MANAGED_INVALID: {0}")]
    Managed(String),
}
