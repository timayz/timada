//! The in-flight sign-in state. oauth2-passkey keeps it in a memory/Redis
//! cache; here each flow travels in one private (encrypted, authenticated)
//! cookie, so any node can finish what another started and nothing has to
//! expire server-side. Each store is read once and removed: a flow cookie is
//! single-use.

use serde::{Deserialize, Serialize};
use timada_identity::ChallengePurpose;
use topcoat::{
    context::Cx,
    cookie::{Cookies, SameSite, UnparsedCookieStore, cookie_store, private_cookies, time},
};
use webauthn_rs::prelude::{DiscoverableAuthentication, PasskeyRegistration};

/// Longest a sign-in attempt may take between its first and last step.
pub const FLOW_MAX_AGE_SECS: u64 = 10 * 60;
/// How long an invitation link waits in the browser for the person to sign
/// in or create an account before it has to be opened again.
pub const INVITE_MAX_AGE_SECS: u64 = 60 * 60;
/// How long a freshly created store API token stays readable on the keys
/// page after the redirect that follows its creation.
pub const API_KEY_REVEAL_MAX_AGE_SECS: u64 = 5 * 60;

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
    /// The signed-in user the passkey is for.
    pub bound_user_id: String,
    pub state: PasskeyRegistration,
    pub issued_at: u64,
}

/// What a mailed code is for, as the pages see it. Adding an address to an
/// account proves the same thing as registering one (a free address whose
/// mail the person reads), so both issue a `Register` code: the domain's
/// [`ChallengePurpose`] is frozen in the event store and the difference only
/// matters to the page the code comes back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailFlowKind {
    Register,
    SignIn,
    ResetPassword,
    /// A signed-in user adds the e-mail + password method.
    AddEmail,
}

impl EmailFlowKind {
    pub fn purpose(self) -> ChallengePurpose {
        match self {
            Self::Register | Self::AddEmail => ChallengePurpose::Register,
            Self::SignIn => ChallengePurpose::SignIn,
            Self::ResetPassword => ChallengePurpose::ResetPassword,
        }
    }
}

/// A mailed code on its way: which address, why, and what to do once it is
/// back. Unlike the other flows this cookie is not single-use: a wrong code
/// keeps it, and it is removed on success, expiry or too many attempts (the
/// attempts themselves are counted server-side).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailFlow {
    pub kind: EmailFlowKind,
    /// Normalized address.
    pub email: String,
    /// `Register`, `AddEmail`: the hash of the password typed on the form.
    pub password_hash: Option<String>,
    /// `SignIn`: the user the password identified. `AddEmail`: the user the
    /// address is for.
    pub user_id: Option<String>,
    pub issued_at: u64,
}

/// An invitation link opened while signed out. Any sign-in or registration
/// that finishes while it is fresh lands back on the invitation page, where
/// the person accepts explicitly.
#[derive(Clone, Serialize, Deserialize)]
pub struct InviteFlow {
    pub token: String,
    pub issued_at: u64,
}

impl std::fmt::Debug for InviteFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InviteFlow")
            .field("token", &"******")
            .field("issued_at", &self.issued_at)
            .finish()
    }
}

/// A passkey sign-in ceremony in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyLoginFlow {
    pub state: DiscoverableAuthentication,
    pub issued_at: u64,
}

/// A store API token on its way from the creating request to the page that
/// shows it once. Private like the sign-in flows: the token never appears in
/// a URL or a log.
#[derive(Clone, Serialize, Deserialize)]
pub struct ApiKeyReveal {
    pub key_id: String,
    pub label: String,
    pub token: String,
    pub issued_at: u64,
}

impl std::fmt::Debug for ApiKeyReveal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyReveal")
            .field("key_id", &self.key_id)
            .field("label", &self.label)
            .field("token", &"******")
            .field("issued_at", &self.issued_at)
            .finish()
    }
}

/// An owner on their way to AliExpress to connect the company's account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliexpressFlow {
    pub company_id: String,
    /// The owner who started it; the callback requires the same one.
    pub user_id: String,
    /// `state` sent to AliExpress; must come back unchanged.
    pub state: String,
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

pub fn email_flow(cx: &Cx) -> UnparsedCookieStore<EmailFlow, impl Cookies + '_> {
    cookie_store(flow_jar(cx), "auth_email")
}

pub fn passkey_login_flow(cx: &Cx) -> UnparsedCookieStore<PasskeyLoginFlow, impl Cookies + '_> {
    cookie_store(flow_jar(cx), "auth_pk_auth")
}

pub fn invite_flow(cx: &Cx) -> UnparsedCookieStore<InviteFlow, impl Cookies + '_> {
    cookie_store(
        flow_jar(cx).default_max_age(time::Duration::seconds(INVITE_MAX_AGE_SECS as i64)),
        "auth_invite",
    )
}

pub fn api_key_reveal(cx: &Cx) -> UnparsedCookieStore<ApiKeyReveal, impl Cookies + '_> {
    cookie_store(
        flow_jar(cx).default_max_age(time::Duration::seconds(API_KEY_REVEAL_MAX_AGE_SECS as i64)),
        "api_key_reveal",
    )
}

pub fn aliexpress_flow(cx: &Cx) -> UnparsedCookieStore<AliexpressFlow, impl Cookies + '_> {
    cookie_store(flow_jar(cx), "aliexpress_flow")
}

/// Whether a flow started at `issued_at` may still finish at `now`.
pub fn is_fresh(issued_at: u64, now: u64) -> bool {
    now >= issued_at && now - issued_at <= FLOW_MAX_AGE_SECS
}

/// Whether an invitation opened at `issued_at` still waits at `now`.
pub fn is_invite_fresh(issued_at: u64, now: u64) -> bool {
    now >= issued_at && now - issued_at <= INVITE_MAX_AGE_SECS
}
