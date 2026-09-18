#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("an admin with email `{0}` already exists")]
    EmailTaken(String),
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error("password hashing failed")]
    Password,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
