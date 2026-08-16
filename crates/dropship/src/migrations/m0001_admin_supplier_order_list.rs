use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001AdminSupplierOrderList;

sqlite_migration!(
    M0001AdminSupplierOrderList,
    "dropship",
    "m0001_admin_supplier_order_list",
    vec_box![],
    vec_box![(
        "CREATE TABLE admin_supplier_order_list (
            id TEXT PRIMARY KEY,
            order_id TEXT NOT NULL,
            supplier_id TEXT NOT NULL,
            external_ref TEXT,
            status TEXT NOT NULL,
            reason TEXT,
            created_at INTEGER NOT NULL
        )",
        "DROP TABLE admin_supplier_order_list"
    )]
);
