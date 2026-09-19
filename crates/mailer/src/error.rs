#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("invalid mailbox `{0}`")]
    InvalidMailbox(String),
    #[error("transport failure: {0}")]
    Transport(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
