use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001AdminShipmentList;

sqlite_migration!(
    M0001AdminShipmentList,
    "shipping",
    "m0001_admin_shipment_list",
    vec_box![],
    vec_box![(
        "CREATE TABLE admin_shipment_list (
            id TEXT PRIMARY KEY,
            order_id TEXT NOT NULL,
            supplier_id TEXT NOT NULL,
            external_ref TEXT NOT NULL,
            tracking_number TEXT,
            carrier TEXT,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        "DROP TABLE admin_shipment_list"
    )]
);
