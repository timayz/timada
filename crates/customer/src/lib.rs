//! Timada customers.
//!
//! A [`Customer`] is the domain identity a shopper can attach orders to. The
//! identity itself is event-sourced — who registered, with what profile — but
//! the credentials are not: the argon2 hash lives in the `customer_credentials`
//! SQL table (via `timada-auth`) so it can be reset and re-hashed, which an
//! immutable event never could. Registration writes the credentials row first
//! (its UNIQUE email is the atomic duplicate gate) and the event second; a
//! crash in between leaves a claimable-but-eventless email, repaired by the
//! best-effort cleanup or manually.
//!
//! Guest checkout stays first-class: nothing here is required to buy. Signing
//! in only attaches the resulting orders to the customer (`timada-order` reads
//! [`current_customer`] at checkout).

mod aggregate;
mod commands;
mod current;
mod migrations;
mod routes;
mod state;
mod view;

pub use aggregate::{Customer, CustomerRegistered};
pub use commands::{LoginError, RegisterError, login_customer, register_customer};
pub use current::current_customer;
pub use migrations::migrations;
pub use routes::store_router;
pub use state::CustomerState;
pub use view::{CustomerView, load_customer};
