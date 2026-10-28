//! Automations bounded context: the scenarios a shop's team draws, each a
//! *trigger* (something that happens in the shop: an order placed, a review
//! submitted, …) followed by *steps* (wait, branch on a condition, notify
//! the team, …), laid out as a tree: a branching step holds a lane of steps
//! per branch.
//!
//! A scenario is written as a [`Definition`], kept whole as JSON in the
//! scenario's events: the merchant edits a working copy and saves it as a
//! draft, and publishing freezes the draft as the next version. Kinds of
//! triggers and steps are strings named by the [`catalog`], and a step's
//! settings are kept exactly as typed, so a draft never loses what was
//! written and a kind this build does not know survives an edit.
//!
//! The crate is pure domain code: no permission checks, no HTTP, no mail.
//! What a step does to the world is the host's.

pub mod aggregator;
pub mod catalog;
mod command;
mod definition;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use definition::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
