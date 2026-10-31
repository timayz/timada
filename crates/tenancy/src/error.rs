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
    #[error("pick a time zone from the list")]
    InvalidTimeZone,
    #[error("not allowed")]
    Forbidden,
    #[error("a publishable key starts with pk_")]
    InvalidPublishableKey,
    #[error("give a two-letter country, a three-letter currency and a two-letter language")]
    InvalidAliexpressMarket,
    #[error("a price rule multiplies by 1 to 100 and adds at most 10,000")]
    InvalidPricingRule,
    #[error("set up the AliExpress app first")]
    AliexpressNotConfigured,
    #[error(
        "give the mail server's host, port and security, the address mails come from, and a name of at most 80 characters"
    )]
    InvalidMailServer,
    #[error("a shop keeps at most 10 mail servers")]
    TooManyMailServers,
    #[error("no such mail server")]
    MailServerNotFound,
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

    /// A stable code for the refusal, for a page to word in its own
    /// language; `None` for an infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidSlug => "invalid_slug",
            Self::SlugTaken => "slug_taken",
            Self::InvalidName => "invalid_name",
            Self::InvalidTimeZone => "invalid_time_zone",
            Self::NotFound => "tenant_not_found",
            Self::Forbidden => "not_allowed",
            Self::Suspended => "tenant_suspended",
            Self::NotSuspended => "not_suspended",
            Self::InvalidRole => "invalid_role",
            Self::ReservedRole => "reserved_role",
            Self::InvalidRoleLabel => "invalid_role_label",
            Self::RoleExists => "role_exists",
            Self::UnknownRole => "unknown_role",
            Self::RoleInUse => "role_in_use",
            Self::InvalidPermission => "invalid_permission",
            Self::InvalidEmail => "invalid_email",
            Self::AlreadyMember => "already_member",
            Self::NotMember => "not_member",
            Self::LastOwner => "last_owner",
            Self::InvitationInvalid => "invitation_invalid",
            Self::InvalidApiKeyLabel => "invalid_api_key_label",
            Self::InvalidApiKeyScopes => "invalid_api_key_scopes",
            Self::ApiKeyInvalid => "api_key_invalid",
            Self::InvalidPublishableKey => "invalid_publishable_key",
            Self::InvalidAliexpressMarket => "invalid_aliexpress_market",
            Self::InvalidPricingRule => "invalid_pricing_rule",
            Self::AliexpressNotConfigured => "aliexpress_not_configured",
            Self::InvalidMailServer => "invalid_mail_server",
            Self::TooManyMailServers => "too_many_mail_servers",
            Self::MailServerNotFound => "mail_server_not_found",
            Self::UnknownMail => "unknown_mail",
            Self::InvalidMailLanguages => "invalid_mail_languages",
            Self::UnknownLanguage => "unknown_language",
            Self::InvalidMailBranding => "invalid_mail_branding",
            Self::InvalidMailTemplate(_) => "invalid_mail_template",
            Self::InvalidSiret => "invalid_siret",
            Self::InvalidVatNumber => "invalid_vat_number",
            Self::OssWithoutVatNumber => "oss_without_vat_number",
            Self::CompanyNotFound => "company_not_found",
            Self::SirenTaken => "siren_taken",
            Self::SirenMismatch => "siren_mismatch",
            Self::NotCompanyOwner => "not_company_owner",
            Self::LastCompanyOwner => "last_company_owner",
            Self::SeriesCodeTaken => "series_code_taken",
            Self::InvalidSeriesCode => "invalid_series_code",
            Self::InvalidLegalIdentity => "invalid_legal_identity",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
