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
    #[error("a publishable key starts with pk_")]
    InvalidPublishableKey,
    #[error("an AliExpress app key is 1 to 64 letters and digits")]
    InvalidAliexpressKey,
    #[error("give a two-letter country, a three-letter currency and a two-letter language")]
    InvalidAliexpressMarket,
    #[error("a price rule multiplies by 1 to 100 and adds at most 10,000")]
    InvalidPricingRule,
    #[error("set up the AliExpress app first")]
    AliexpressNotConfigured,
    #[error("give the mail server's host, port and security, and the address mails come from")]
    InvalidMailServer,
    #[error("unknown mail")]
    UnknownMail,
    #[error("pick the languages from the list")]
    InvalidMailLanguages,
    #[error("this shop does not mail in that language")]
    UnknownLanguage,
    #[error("an accent colour is #rrggbb, and a footer at most 500 characters")]
    InvalidMailBranding,
    #[error(transparent)]
    InvalidMailTemplate(#[from] crate::MailTemplateError),
    #[error("a SIRET is 14 digits that pass the check")]
    InvalidSiret,
    #[error("the VAT number must be FR, its two-digit key and the SIREN")]
    InvalidVatNumber,
    #[error("the One-Stop Shop needs a VAT number")]
    OssWithoutVatNumber,
    #[error("company not found")]
    CompanyNotFound,
    #[error("a company with this SIREN is already registered: ask its owner to add you")]
    SirenTaken,
    #[error("the SIRET of a company keeps its SIREN")]
    SirenMismatch,
    #[error("only an owner of the company may do this")]
    NotCompanyOwner,
    #[error("a company keeps at least one owner")]
    LastCompanyOwner,
    #[error("another tenant of the company numbers its invoices under this code")]
    SeriesCodeTaken,
    #[error("a series code is up to 10 letters and digits")]
    InvalidSeriesCode,
    #[error("fill in every required part of the legal identity, within its length")]
    InvalidLegalIdentity,
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
    #[error("choose at least one known scope for the key")]
    InvalidApiKeyScopes,
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
        match self {
            // A write the store is not taking right now is told to the person.
            Self::Write(err) => timada_core::write_refusal(err).is_some(),
            Self::Other(_) => false,
            _ => true,
        }
    }
}
