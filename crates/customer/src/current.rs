//! Resolve the browser's session cookie to a customer, if any.

use axum_extra::extract::cookie::CookieJar;
use timada_auth::{SessionKind, load_session, session_token};

use crate::state::CustomerState;
use crate::view::{CustomerView, load_customer};

/// The signed-in customer this request belongs to, or `None` for guests,
/// expired sessions and admin-only sessions alike.
pub async fn current_customer(
    state: &CustomerState,
    jar: &CookieJar,
) -> anyhow::Result<Option<CustomerView>> {
    let Some(token) = session_token(jar) else {
        return Ok(None);
    };
    let Some(session) = load_session(&state.auth.read_pool, &token, SessionKind::Customer).await?
    else {
        return Ok(None);
    };

    load_customer(&state.ctx.executor, &session.subject_id).await
}
