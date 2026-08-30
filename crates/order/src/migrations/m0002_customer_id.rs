use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0002CustomerId;

sqlite_migration!(
    M0002CustomerId,
    "order",
    "m0002_customer_id",
    vec_box![("order", "m0001_admin_order_list")],
    vec_box![(
        "ALTER TABLE admin_order_list ADD COLUMN customer_id TEXT",
        "ALTER TABLE admin_order_list DROP COLUMN customer_id"
    )]
);
