//! Read side. The write models double as views for the account page (a
//! user's own stream is read directly, so the login redirect never waits on
//! a projection). Listing *across* users needs an index a relational store
//! is good at: [`read_model_subscription`] keeps two SQLite tables current
//! from the event stream, and [`users`] queries them.

mod migrations;
mod read_model;
mod users;

pub use crate::command::UserState as AccountView;
pub use migrations::*;
pub use read_model::*;
pub use users::*;
