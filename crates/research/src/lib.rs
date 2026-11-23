//! Research bounded context: the niche studies a user keeps before any shop
//! exists.
//!
//! A study is a candidate niche: a name, the seed keywords it starts from,
//! and the country and language they are searched in. It belongs to the
//! user who made it, as a company belongs to its owners, and its stream
//! sits in the platform's own tenant (the default tenant of the store):
//! nothing of it is a shop's.
//!
//! The keywords of a seed are fetched off the page that asks for them: the
//! request is an event, the host's saga calls the keyword source and records
//! what it gave, or why it gave nothing, on the study. The rows of a fetch
//! live in the event that records it, so a rebuilt read model and every
//! node of a cluster hold the same rows. The crate is pure domain code: it
//! never calls the source, and knows nothing of who it is.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
