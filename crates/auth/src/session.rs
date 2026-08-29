//! DB-backed sessions with an unguessable bearer token in an HttpOnly cookie.
//!
//! The token is 256 bits from the OS RNG, so signing it would add nothing —
//! the value itself is the secret, and keeping it server-side means logout and
//! revocation actually delete something. The same reasoning keeps the cookie
//! unsigned, consistent with the cart cookie.

use argon2::password_hash::rand_core::{OsRng, RngCore as _};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use sqlx::SqlitePool;

/// Name of the cookie holding the session token.
pub const SESSION_COOKIE: &str = "timada_session";

/// How long an admin session lives: 30 days.
pub const ADMIN_SESSION_TTL_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Who a session authenticates. Stored as a column so an admin token can never
/// be replayed as a customer or vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Admin,
    Customer,
}

impl SessionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Customer => "customer",
        }
    }
}

/// A live (non-expired) session row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub token: String,
    pub subject_id: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Current wall-clock time as epoch milliseconds, the workspace's timestamp
/// convention.
pub fn now_millis() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        // A clock before 1970 yields the epoch; sessions created then simply
        // read as expired, which fails safe.
        Err(_) => 0,
    }
}

/// 256 bits from the OS RNG, hex-encoded.
fn new_session_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut token = String::with_capacity(64);
    for byte in bytes {
        // Infallible: writing hex digits into a String.
        use std::fmt::Write as _;
        let _ = write!(token, "{byte:02x}");
    }
    token
}

/// Create a session for `subject_id` and return its token. Expired sessions
/// are swept lazily here — logins are rare enough to absorb the housekeeping
/// and frequent enough to keep the table small.
pub async fn create_session(
    write_pool: &SqlitePool,
    subject_id: &str,
    kind: SessionKind,
    ttl_millis: i64,
) -> anyhow::Result<String> {
    let now = now_millis();
    sqlx::query("DELETE FROM auth_sessions WHERE expires_at < ?")
        .bind(now)
        .execute(write_pool)
        .await?;

    let token = new_session_token();
    sqlx::query(
        "INSERT INTO auth_sessions (token, subject_id, kind, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(subject_id)
    .bind(kind.as_str())
    .bind(now)
    .bind(now.saturating_add(ttl_millis))
    .execute(write_pool)
    .await?;

    Ok(token)
}

/// Load a session by token, `None` when unknown, expired, or the wrong kind.
pub async fn load_session(
    read_pool: &SqlitePool,
    token: &str,
    kind: SessionKind,
) -> anyhow::Result<Option<Session>> {
    let session = sqlx::query_as::<_, Session>(
        "SELECT token, subject_id, created_at, expires_at
         FROM auth_sessions
         WHERE token = ? AND kind = ? AND expires_at >= ?",
    )
    .bind(token)
    .bind(kind.as_str())
    .bind(now_millis())
    .fetch_optional(read_pool)
    .await?;

    Ok(session)
}

/// Revoke one session. Missing tokens are a no-op — logout must be idempotent.
pub async fn delete_session(write_pool: &SqlitePool, token: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM auth_sessions WHERE token = ?")
        .bind(token)
        .execute(write_pool)
        .await?;
    Ok(())
}

/// The session token this browser carries, if any.
pub fn session_token(jar: &CookieJar) -> Option<String> {
    jar.get(SESSION_COOKIE)
        .map(|cookie| cookie.value().to_owned())
}

/// Build the session cookie. `HttpOnly` because no script needs the token,
/// `SameSite=Lax` so a normal navigation stays logged in without the cookie
/// riding on cross-site POSTs, path `/` because both the storefront account
/// pages and the admin read it. Deliberately a browser-session cookie, like
/// the cart cookie: the authoritative lifetime is the row's `expires_at`, and
/// a token the browser forgot early costs one login, while a `Max-Age` would
/// add a `time` dependency for a promise the server already keeps.
pub fn session_cookie(token: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .same_site(SameSite::Lax)
        .http_only(true)
        .build()
}

/// A cookie that removes the session cookie.
pub fn clear_session_cookie() -> Cookie<'static> {
    let mut cookie = Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .same_site(SameSite::Lax)
        .http_only(true)
        .build();
    cookie.make_removal();
    cookie
}
