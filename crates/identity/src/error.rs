use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("this account is not registered")]
    NotRegistered,
    #[error("this account is already registered")]
    AlreadyRegistered,
    #[error("this account is already linked to another user")]
    AlreadyLinkedToOtherUser,
    #[error("the session changed since this step was started")]
    SessionMismatch,
    #[error("already signed in")]
    UnexpectedlyAuthorized,
    #[error("sign in required")]
    Unauthorized,
    #[error("this request does not match the current sign-in state")]
    InvalidState,
    #[error("the last sign-in method cannot be removed")]
    LastSignInMethod,
    #[error("unknown passkey")]
    UnknownCredential,
    #[error("user not found")]
    UserNotFound,
    #[error("too many passkeys on this account")]
    TooManyPasskeys,
    #[error("not allowed")]
    Forbidden,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IdentityError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::Write(_) | Self::Other(_))
    }
}
