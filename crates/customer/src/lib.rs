//! Customer bounded context: identity and the address book (one billing
//! address, several delivery addresses with a preferred one). Credentials
//! and sessions are never events and do not live here.

pub mod aggregator;
mod command;
mod error;
mod migration;
mod query;
mod read_model;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use query::*;
pub use read_model::*;
pub use value_object::*;
