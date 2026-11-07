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
//!
//! A sign-in identity lives in two streams, its index entry and the user's,
//! written one after the other. An entry counts only while the account it
//! points at lists what it indexes: one left behind by a crash in between
//! signs nobody in and may be claimed again, and
//! [`index_release_subscription`] frees those a user let go of.

pub mod aggregator;
mod command;
mod error;
mod index_release;
mod password;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use index_release::*;
pub use password::*;
pub use query::*;
pub use timada_core::{CodeVerifier, HmacCodeVerifier};
pub use value_object::*;
