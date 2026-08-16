//! Read-side schema for this crate. The event store's own schema belongs to
//! evento and is migrated separately by [`timada_core::ServiceContext::new`].

use sqlx::Sqlite;
use sqlx_migrator::migration::Migration;
use sqlx_migrator::vec_box;

mod m0001_product_read_models;

/// Register these with the application's `Migrator` alongside every other
/// service crate's migrations.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![m0001_product_read_models::M0001ProductReadModels]
}
