//! Writer bounded context: first drafts of a shop's texts, written by a
//! model for a person to correct.
//!
//! A member asks for the texts of a product or a category to be drafted:
//! a name, a description, and what search engines show. The request is
//! recorded here; the host asks the model off the page that asked, and
//! what comes back is never published: it is prepared as a proposal
//! (`timada-proposals`), which waits for a member to edit, approve or
//! refuse it. What became of the request is recorded here too, with what
//! the call cost and what in the draft could not be found in the shop's
//! own data.
//!
//! A shop says once how it wants to be written for ([`Brief`]): its tone,
//! and what a text must never claim.
//!
//! Every call is paid by the platform, so a day has two caps, one per shop
//! and one for every shop together ([`SpendDay`], in the platform's own
//! tenant). Room is held for a call before it is made, at the most it may
//! cost, so a cap is never passed.
//!
//! The crate is pure domain code: it never calls a model and never writes
//! a product.
//!
//! [`Brief`]: aggregator::Brief
//! [`SpendDay`]: aggregator::SpendDay

pub mod aggregator;
mod command;
mod error;
pub mod facts;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
