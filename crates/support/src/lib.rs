//! Support bounded context: the messages shoppers send the shop through its
//! contact form.
//!
//! A message is what a visitor wrote, signed with their name and the address
//! to answer at; the merchant reads it in the back office and marks it
//! handled once answered (by mail, outside the platform). Messages live
//! under the shop's tenant in the store and the crate is pure domain code: no
//! permission checks, no HTTP, no rate limiting (whoever takes the form
//! decides how often one visitor may send).
//!
//! Events cannot be erased, so they never hold what the visitor wrote in
//! clear: each message draws a random key ([`timada_core::Key`]) and its
//! name, address and text are recorded sealed under it. The crate never
//! stores a key: the caller keeps them through [`timada_core::Keys`] and
//! destroys the message's key when the merchant erases it
//! ([`Command::forget_message`]), which leaves what the message recorded
//! unreadable for good (crypto-shredding). The language is kept in clear.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
