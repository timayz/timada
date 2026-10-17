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
    #[error("the e-mail address or the password is wrong")]
    InvalidCredentials,
    #[error("this code is not right")]
    InvalidCode,
    #[error("this code has expired")]
    CodeExpired,
    #[error("too many wrong codes; ask for a new one")]
    TooManyAttempts,
    #[error("a code was sent a moment ago; wait before asking for another")]
    CodeRecentlySent,
    #[error("this is not a valid e-mail address")]
    InvalidEmail,
    #[error("the password must be between 8 and 128 characters")]
    WeakPassword,
    #[error("the current password is wrong")]
    WrongPassword,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IdentityError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        match self {
            // A write the store is not taking right now is told to the person.
            Self::Write(err) => timada_core::write_refusal(err).is_some(),
            Self::Other(_) => false,
            _ => true,
        }
    }
}
