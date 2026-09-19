//! Review bounded context: customer reviews (star rating + moderation) and
//! product questions/answers.

pub mod aggregator;
mod command;
mod error;
mod migration;
mod query;
mod read_model;
mod review_list;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use query::*;
pub use read_model::*;
pub use review_list::*;
pub use value_object::*;
