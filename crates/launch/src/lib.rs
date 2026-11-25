//! Launch bounded context: what a new shop set out to reach, and the dates
//! its numbers are read at to say whether to go on.
//!
//! A new shop loses money for weeks by design. Before its first campaign
//! the merchant sets a target: a monthly profit after ads, and a date. Four
//! checkpoints follow, counted in days from the first campaign's start
//! (21, 45, 90, 120): at each the host reads the numbers timada holds and
//! records them here with what they say (go on, fix, or stop), on
//! thresholds the shop may change. The merchant then records what they
//! decided, and why. A "stop" is advice: nothing here switches anything
//! off.
//!
//! Everything lives on one stream a shop, in the shop's own tenant: the
//! target and each change of it, the thresholds, each checkpoint as it was
//! read and each decision. The rules ([`judge`]) are pure arithmetic; the
//! crate never reads a number itself and never sends a mail.

pub mod aggregator;
mod command;
mod error;
mod value_object;

pub use command::*;
pub use error::*;
pub use value_object::*;
