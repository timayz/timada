//! The admin gate. Wrap the protected admin nest with this via
//! `axum::middleware::from_fn_with_state(auth_state, require_admin)`; the
//! login routes themselves stay outside the nest.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::CookieJar;

use crate::session::{SessionKind, load_session, session_token};
use crate::state::AuthState;

/// Path unauthenticated admin requests are redirected to.
pub const LOGIN_PATH: &str = "/admin/login";

/// Pass the request through only when the browser carries a live admin
/// session; anything else is redirected to the login form. A DB failure is a
/// 500, not a redirect — an outage must not read as "logged out".
pub async fn require_admin(
    State(state): State<AuthState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Response {
    let Some(token) = session_token(&jar) else {
        return Redirect::to(LOGIN_PATH).into_response();
    };

    match load_session(&state.read_pool, &token, SessionKind::Admin).await {
        Ok(Some(_)) => next.run(req).await,
        Ok(None) => Redirect::to(LOGIN_PATH).into_response(),
        Err(source) => {
            tracing::error!(error = ?source, "failed to load admin session");
            timada_core::AppError::Internal(source).into_response()
        }
    }
}
