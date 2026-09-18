//! Invoice bounded context: one invoice per order, drafted at placement,
//! issued (numbered from a write-side SQL sequence) once paid, voided when
//! the order is cancelled.

pub mod aggregator;
mod command;
mod error;
mod migration;
mod process;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use process::*;
pub use query::*;
pub use value_object::*;
