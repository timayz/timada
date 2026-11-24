//! Proposals bounded context: a change to a shop that something prepared
//! and a person has to read before it applies.
//!
//! A rule, a model writing texts, an agent, or a member who may not change
//! the catalog themselves, prepares a proposal: what would change (each
//! field with its value when the proposal was made and the value it would
//! take), why, who prepared it and until when it is worth reading. It waits
//! on the shop until a member who may approve says yes, as it is or after
//! editing what it would write, or no.
//!
//! Approving changes nothing here: it records the yes, with the values
//! approved and who approved them, and the host applies them through the
//! command a person would have used, under that person's name, then says
//! here how it went. A proposal whose target changed since it was prepared
//! is not applied over the newer value: it ends as out of date.
//!
//! A proposal names its target by a kind, an id and field names, all plain
//! strings, so a new kind of change needs no new event. The crate knows the
//! shape and the life of a proposal; what a kind's fields mean, and how to
//! read and write them, is the host's.
//!
//! The streams live in the shop's own tenant. The crate is pure domain
//! code: it never changes a product and never sends a mail.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
