//! The in-flight sign-in state. oauth2-passkey keeps it in a memory/Redis
//! cache; here each flow travels in one private (encrypted, authenticated)
//! cookie, so any node can finish what another started and nothing has to
//! expire server-side. Each store is read once and removed: a flow cookie is
//! single-use.

use serde::{Deserialize, Serialize};
use topcoat::{
    context::Cx,
    cookie::{Cookies, SameSite, UnparsedCookieStore, cookie_store, private_cookies, time},
};
use webauthn_rs::prelude::{DiscoverableAuthentication, PasskeyRegistration};

/// Longest a sign-in attempt may take between its first and last step.
pub const FLOW_MAX_AGE_SECS: u64 = 10 * 60;

/// An OIDC redirect in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcFlow {
    pub provider: String,
    pub mode: String,
    /// `state` sent to the provider; must come back unchanged.
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
    /// The user signed in when the flow started, if any. For `add_to_user`
    /// the callback requires the same user to still be signed in.
    pub bound_user_id: Option<String>,
    /// Unix seconds.
    pub issued_at: u64,
}

/// A passkey registration ceremony in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyRegistrationFlow {
    pub mode: String,
    /// The user the passkey is for: the signed-in user, or the id a new user
    /// will get.
    pub bound_user_id: String,
    pub account: String,
    pub label: String,
    pub state: PasskeyRegistration,
    pub issued_at: u64,
}

/// A passkey sign-in ceremony in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyLoginFlow {
    pub state: DiscoverableAuthentication,
    pub issued_at: u64,
}

fn flow_jar(cx: &Cx) -> impl Cookies + '_ {
    // `Lax`, not `Strict`: the OIDC callback is a top-level cross-site
    // navigation and must still carry the cookie.
    private_cookies(cx)
        .default_same_site(SameSite::Lax)
        .default_path("/")
        .default_http_only(true)
        .default_secure(true)
        .default_max_age(time::Duration::seconds(FLOW_MAX_AGE_SECS as i64))
}

pub fn oidc_flow(cx: &Cx) -> UnparsedCookieStore<OidcFlow, impl Cookies + '_> {
    cookie_store(flow_jar(cx), "auth_oidc")
}

pub fn passkey_registration_flow(
    cx: &Cx,
) -> UnparsedCookieStore<PasskeyRegistrationFlow, impl Cookies + '_> {
    cookie_store(flow_jar(cx), "auth_pk_reg")
}

pub fn passkey_login_flow(cx: &Cx) -> UnparsedCookieStore<PasskeyLoginFlow, impl Cookies + '_> {
    cookie_store(flow_jar(cx), "auth_pk_auth")
}

/// Whether a flow started at `issued_at` may still finish at `now`.
pub fn is_fresh(issued_at: u64, now: u64) -> bool {
    now >= issued_at && now - issued_at <= FLOW_MAX_AGE_SECS
}
