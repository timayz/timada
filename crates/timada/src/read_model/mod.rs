//! Denormalized read models, one per query shape.
//!
//! Tables are created by the migrations in [`migrations`] and kept up to
//! date by event subscriptions. The write side (aggregates) is never queried
//! to render UI.

use sqlx::Sqlite;
use sqlx_migrator::migration::Migration;

/// All read-model migrations (app name `"timada"`), in registration order.
///
/// The evento event-store schema is NOT part of this list — it is owned by
/// `evento::sql_migrator`. [`crate::db::migrate`] runs both.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    // Read-model migrations are registered here as they land.
    Vec::new()
}
