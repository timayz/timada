use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum TenancyError {
    #[error("a slug is 2 to 32 lowercase letters, digits and dashes")]
    InvalidSlug,
    #[error("this slug is already taken")]
    SlugTaken,
    #[error("the name must be between 1 and 80 characters")]
    InvalidName,
    #[error("tenant not found")]
    NotFound,
    #[error("not allowed")]
    Forbidden,
    #[error("this tenant is suspended")]
    Suspended,
    #[error("this tenant is not suspended")]
    NotSuspended,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl TenancyError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::Write(_) | Self::Other(_))
    }
}
