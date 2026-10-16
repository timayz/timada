//! Creating a tenant, under `/tenants`. A tenant's own pages live under
//! `/t/{slug}` (see [`crate::app::t`]).

pub mod new;

use timada_tenancy::{Command, TenancyError};
use topcoat::context::{Cx, app_context};

use crate::cluster::Executor;

/// The tenancy command gateway over this node's platform store.
pub fn tenancy(cx: &Cx) -> Command<Executor> {
    Command(app_context::<Executor>(cx).clone())
}

/// The query-string code the pages show for a domain error.
pub fn error_code(err: &TenancyError) -> &'static str {
    match err {
        TenancyError::InvalidSlug => "invalid_slug",
        TenancyError::SlugTaken => "slug_taken",
        TenancyError::InvalidName => "invalid_name",
        TenancyError::NotFound => "tenant_not_found",
        TenancyError::Forbidden => "not_allowed",
        TenancyError::Suspended => "tenant_suspended",
        TenancyError::NotSuspended => "not_suspended",
        TenancyError::InvalidRole => "invalid_role",
        TenancyError::ReservedRole => "reserved_role",
        TenancyError::InvalidRoleLabel => "invalid_role_label",
        TenancyError::RoleExists => "role_exists",
        TenancyError::UnknownRole => "unknown_role",
        TenancyError::RoleInUse => "role_in_use",
        TenancyError::InvalidPermission => "invalid_permission",
        TenancyError::InvalidEmail => "invalid_email",
        TenancyError::AlreadyMember => "already_member",
        TenancyError::NotMember => "not_member",
        TenancyError::LastOwner => "last_owner",
        TenancyError::InvitationInvalid => "invitation_invalid",
        TenancyError::InvalidApiKeyLabel => "invalid_api_key_label",
        TenancyError::InvalidApiKeyScopes => "invalid_api_key_scopes",
        TenancyError::ApiKeyInvalid => "api_key_invalid",
        TenancyError::InvalidPublishableKey => "invalid_publishable_key",
        TenancyError::InvalidMailServer => "invalid_mail_server",
        TenancyError::UnknownMail => "unknown_mail",
        TenancyError::InvalidMailBranding => "invalid_mail_branding",
        TenancyError::InvalidMailTemplate(_) => "invalid_mail_template",
        TenancyError::InvalidSiret => "invalid_siret",
        TenancyError::InvalidVatNumber => "invalid_vat_number",
        TenancyError::OssWithoutVatNumber => "oss_without_vat_number",
        TenancyError::CompanyNotFound => "company_not_found",
        TenancyError::SirenTaken => "siren_taken",
        TenancyError::SirenMismatch => "siren_mismatch",
        TenancyError::NotCompanyOwner => "not_company_owner",
        TenancyError::LastCompanyOwner => "last_company_owner",
        TenancyError::SeriesCodeTaken => "series_code_taken",
        TenancyError::InvalidSeriesCode => "invalid_series_code",
        TenancyError::InvalidLegalIdentity => "invalid_legal_identity",
        TenancyError::Write(_) | TenancyError::Other(_) => "failed",
    }
}
