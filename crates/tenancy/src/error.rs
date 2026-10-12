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
    #[error("a role name is 2 to 32 lowercase letters, digits and dashes")]
    InvalidRole,
    #[error("this role name is reserved")]
    ReservedRole,
    #[error("a role label is 1 to 40 characters")]
    InvalidRoleLabel,
    #[error("this role already exists")]
    RoleExists,
    #[error("unknown role")]
    UnknownRole,
    #[error("this role is still held by a member")]
    RoleInUse,
    #[error("unknown permission")]
    InvalidPermission,
    #[error("invalid e-mail address")]
    InvalidEmail,
    #[error("this user is already a member")]
    AlreadyMember,
    #[error("this user is not a member")]
    NotMember,
    #[error("a tenant keeps at least one owner")]
    LastOwner,
    #[error("this invitation is no longer valid")]
    InvitationInvalid,
    #[error("a key label is 1 to 40 characters")]
    InvalidApiKeyLabel,
    #[error("this API key is not valid")]
    ApiKeyInvalid,
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
