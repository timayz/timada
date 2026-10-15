//! Back-office pages under `/admin`, for administrators only.

pub mod companies;
pub mod tenants;
pub mod users;

use timada_identity::{IdentityError, UserState};
use topcoat::{Result, context::Cx, router::error::forbidden};

use crate::auth::require_user;

/// The signed-in administrator; a redirect to the login page for visitors
/// and 403 for members.
pub async fn require_admin(cx: &Cx) -> Result<&UserState> {
    let user = require_user(cx).await?;
    if !user.is_admin {
        return Err(forbidden().into());
    }
    Ok(user)
}

/// Domain errors of an admin action, as the query-string code of the page the
/// action redirects back to.
pub fn error_code(err: &IdentityError) -> &'static str {
    crate::app::auth::error_code(err)
}
