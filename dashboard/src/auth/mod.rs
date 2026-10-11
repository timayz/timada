//! Request-side glue for the identity bounded context: who the current
//! request is, the protocol adapters (OIDC, WebAuthn) and the encrypted
//! cookies that carry a sign-in attempt between its steps.

pub mod cookies;
pub mod oidc;
pub mod webauthn;

use std::time::{SystemTime, UNIX_EPOCH};

use timada_identity::{Command, LoginContext, UserState};
use topcoat::{
    Result,
    context::{Cx, app_context, memoize},
    router::{
        error::RouterErrorExt,
        href,
        request::{client_ip, headers},
    },
    session,
};

use crate::cluster::Executor;

/// The command gateway over this node's executor.
pub fn identity(cx: &Cx) -> Command<Executor> {
    Command(app_context::<Executor>(cx).clone())
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The signed-in user, resolved once per request from the session cookie.
/// `None` when there is no session, it expired, or the user is gone.
#[memoize(as_ref)]
pub async fn current_user(cx: &Cx) -> Option<UserState> {
    let hash = match session::token_hash(cx).await {
        Ok(Some(hash)) => hash,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(%err, "cannot read the session token");
            return None;
        }
    };
    match identity(cx).resolve_session(&hash, now()).await {
        Ok(user) => user,
        Err(err) => {
            tracing::error!(%err, "cannot resolve the session");
            None
        }
    }
}

/// The signed-in user, or a redirect to the login page.
pub async fn require_user(cx: &Cx) -> Result<&UserState> {
    Ok(current_user(cx)
        .await
        .ok_or_redirect(href!(crate::app::auth::login::login).resolve(cx))?)
}

/// Where this request comes from, for the sign-in history.
pub fn login_context(cx: &Cx) -> LoginContext {
    LoginContext {
        user_agent: headers(cx)
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .map(|ua| ua.chars().take(512).collect()),
        ip: client_ip(cx).map(|ip| ip.to_string()),
    }
}

/// Starts a browser session for `user_id` and records it. Call after a
/// successful sign-in or registration.
pub async fn start_session(cx: &Cx, user_id: &str) -> Result<()> {
    let session = session::start(cx).await?;
    identity(cx)
        .start_session(
            &session.token_hash,
            user_id,
            unix_seconds(session.expires_at),
            login_context(cx),
        )
        .await?;
    Ok(())
}
