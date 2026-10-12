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
        TenancyError::Write(_) | TenancyError::Other(_) => "failed",
    }
}
