//! Core domain for timada e-commerce stores.
//!
//! Event-sourced with [`evento`]: aggregates and commands write to the event
//! store; denormalized read models in [`read_model`] serve every query.
//!
//! ⚠️ This package MUST keep the name `timada` forever: evento persists each
//! aggregate's type as `"{cargo_pkg_name}/{EnumName}"`, so renaming the crate
//! orphans every event already written to a store's database.

pub mod db;
pub mod provider_connection;
pub mod read_model;
pub mod subscriptions;
