//! Read-side migrations owned by the order context.
//!
//! The event store's own schema is managed by evento — never here.

use sqlx::Sqlite;
use sqlx_migrator::migration::Migration;
use sqlx_migrator::vec_box;

mod m0001_admin_order_list;

pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![m0001_admin_order_list::M0001AdminOrderList]
}
