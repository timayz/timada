//! Admin identity: users and sessions live in SQL (credentials are never
//! events). topcoat's session layer hands out a token hash; the rows that tie
//! a hash to an admin are ours.

mod password;
mod role;
mod store;
pub mod team;

use topcoat::context::{Cx, app_context, memoize, try_request_context};
use topcoat::session;

use crate::config::AdminServices;

pub use role::{OWN_PASSWORD, Role, Section};
pub use store::{AdminUser, create_admin, create_operator};

/// The admin authenticated for this request, attached by the `_secure` layer.
#[derive(Debug, Clone)]
pub struct CurrentAdmin(pub AdminUser);

/// The admin the request's session belongs to, if any. Memoized per request.
#[memoize(as_ref)]
pub async fn current_admin(cx: &Cx) -> topcoat::Result<Option<AdminUser>> {
    let Some(token_hash) = session::token_hash(cx).await? else {
        return Ok(None);
    };
    let services = app_context::<AdminServices>(cx);
    Ok(store::find_by_session(&services.db, &token_hash).await?)
}

/// The admin attached by the `_secure` layer; `None` outside it.
pub fn signed_in_admin(cx: &Cx) -> Option<&AdminUser> {
    try_request_context::<CurrentAdmin>(cx).map(|c| &c.0)
}

/// Checks the credentials and opens a session: the operator who signed in.
/// `Ok(None)` on a bad email/password pair (no hint which).
pub async fn sign_in(cx: &Cx, email: &str, password: &str) -> topcoat::Result<Option<AdminUser>> {
    let services = app_context::<AdminServices>(cx);
    let Some((admin, hash)) = store::find_credentials(&services.db, email).await? else {
        // Burn comparable time so a missing account is not distinguishable.
        password::verify(password, &password::DUMMY_HASH);
        return Ok(None);
    };
    if !password::verify(password, &hash) {
        return Ok(None);
    }
    let started = session::start(cx).await?;
    store::insert_session(
        &services.db,
        &started.token_hash,
        &admin.id,
        started.expires_at,
    )
    .await?;
    tracing::info!(admin_id = %admin.id, "admin signed in");
    Ok(Some(admin))
}

/// Closes the current session, if any.
pub async fn sign_out(cx: &Cx) -> topcoat::Result<()> {
    let services = app_context::<AdminServices>(cx);
    if let Some(token_hash) = session::stop(cx).await? {
        store::delete_session(&services.db, &token_hash).await?;
    }
    Ok(())
}
