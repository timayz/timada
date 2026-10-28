//! Runs the shops' published automation scenarios. A trigger event (an order
//! placed, a message received, …) starts a run of every scenario listening
//! to it ([`triggers`]); the runner ([`runner`]) then takes each run through
//! its steps on a timer, from every node: each move is written against the
//! run's version, so the node that writes it first is the one that acts.

pub mod definitions;
pub mod facts;
pub mod runner;
pub mod triggers;
