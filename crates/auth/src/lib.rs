//! Timada auth.
//!
//! Password hashing, DB-backed sessions and admin users — deliberately not
//! event-sourced. Credentials must be rotatable and sessions revocable, and an
//! immutable event stream is the wrong home for either: an argon2 hash frozen
//! into an event could never be re-hashed with stronger parameters, and a
//! session that cannot be deleted cannot be revoked. Both live in plain SQL
//! tables instead, the same write-side-state precedent as the invoice sequence
//! counter.
//!
//! One session table serves both audiences: a [`SessionKind`] column keeps an
//! admin session from ever authenticating a customer request or vice versa.
//! `timada-customer` builds its customer sessions on this crate; the admin
//! login UI lives here because admin users do too.
//!
//! Consumers mount [`admin_auth_router`] at the site root (it owns
//! `/admin/login` and `/admin/logout`) and wrap the protected admin nest with
//! [`require_admin`] via `axum::middleware::from_fn_with_state`.

mod admin_users;
mod middleware;
mod migrations;
mod password;
mod routes;
mod session;
mod state;

pub use admin_users::{create_admin_user, verify_admin};
pub use middleware::require_admin;
pub use migrations::migrations;
pub use password::{hash_password, verify_password};
pub use routes::admin_auth_router;
pub use session::{
    ADMIN_SESSION_TTL_MILLIS, SESSION_COOKIE, Session, SessionKind, clear_session_cookie,
    create_session, delete_session, load_session, now_millis, session_cookie, session_token,
};
pub use state::AuthState;
