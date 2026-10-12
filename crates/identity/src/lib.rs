//! Identity bounded context: users, their sign-in methods (OIDC accounts,
//! passkeys, e-mail + password) and their sessions, all event-sourced.
//!
//! The crate is pure domain code: it knows nothing about HTTP, cookies, OIDC
//! token exchange or WebAuthn ceremonies. The web layer verifies the protocol
//! and hands the resulting facts (a provider identity, a verified credential,
//! a session token hash) to [`Command`].
//!
//! Lookups that a relational store would index (`provider + subject → user`,
//! `credential id → user`, `e-mail → user`, `session token → user`) are aggregates with a
//! deterministic id (see [`value_object`]). Appending their first event at
//! version 0 is an atomic create-unless-exists, which doubles as the uniqueness
//! check, and loading them reads the event store directly, so a login sees its
//! own writes without waiting for a projection to catch up.

pub mod aggregator;
mod command;
mod error;
mod password;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use password::*;
pub use query::*;
pub use value_object::*;
