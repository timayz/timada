//! Request-side glue for the identity bounded context: who the current
//! request is, the protocol adapters (OIDC, WebAuthn) and the encrypted
//! cookies that carry a sign-in attempt between its steps.

pub mod cookies;
pub mod oidc;
pub mod webauthn;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use timada_identity::{Command, LoginContext, SessionState, UserState, users_by_ids};
use topcoat::{
    Result,
    context::{Cx, app_context, memoize},
    router::{
        error::RouterErrorExt,
        href,
        request::{client_ip, headers},
    },
    session::{self, TokenHash},
};

use crate::cluster::Executor;
use crate::config::Config;
use crate::db::ReadPool;

/// The command gateway over this node's executor.
pub fn identity(cx: &Cx) -> Command<Executor> {
    Command::new(
        app_context::<Executor>(cx).clone(),
        app_context::<crate::seal::EmailCodes>(cx).0.clone(),
    )
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
/// `None` when there is no session, it expired, was revoked, or the user is
/// gone. A session past half its lifetime is renewed on the way.
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
    let resolved = match identity(cx).resolve_session(&hash, now()).await {
        Ok(Some(resolved)) => resolved,
        Ok(None) => return None,
        Err(err) => {
            tracing::error!(%err, "cannot resolve the session");
            return None;
        }
    };
    slide_expiry(cx, &hash, &resolved.session).await;
    Some(resolved.user)
}

/// Sliding expiration: once half the lifetime has elapsed, re-issue the
/// cookie for a full lifetime and extend the record. Best effort: the
/// request proceeds on the current session whatever happens here.
async fn slide_expiry(cx: &Cx, hash: &TokenHash, session: &SessionState) {
    let lifetime = app_context::<Config>(cx).auth.session_lifetime_secs;
    if session.expires_at.saturating_sub(now()) >= lifetime / 2 {
        return;
    }
    let refreshed = match session::refresh(cx).await {
        Ok(Some(refreshed)) => refreshed,
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(%err, "cannot re-issue the session cookie");
            return;
        }
    };
    if let Err(err) = identity(cx)
        .refresh_session(hash, unix_seconds(refreshed.expires_at))
        .await
    {
        tracing::warn!(%err, "cannot extend the session record");
    }
}

/// Display labels of users, by id, from the read model; an id it does not
/// know (yet) is shown as is.
pub async fn user_labels(cx: &Cx, ids: &[String]) -> Result<HashMap<String, String>> {
    let rows = users_by_ids(app_context::<ReadPool>(cx), ids).await?;
    let mut labels: HashMap<String, String> = rows.into_iter().map(|u| (u.id, u.label)).collect();
    for id in ids {
        labels.entry(id.clone()).or_insert_with(|| id.clone());
    }
    Ok(labels)
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

/// Signs the browser in as `user_id` after a successful sign-in or
/// registration. A session already present (switching accounts, or signing in
/// again) is replaced: its token is rotated and its record ended, so no token
/// ever outlives the user it was issued to.
pub async fn sign_in_as(cx: &Cx, user_id: &str) -> Result<()> {
    let session = match session::rotate(cx).await? {
        Some(rotation) => {
            identity(cx).end_session(&rotation.revoked).await?;
            rotation.session
        }
        None => session::start(cx).await?,
    };
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
