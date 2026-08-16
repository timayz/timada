use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001AdminOrderList;

sqlite_migration!(
    M0001AdminOrderList,
    "order",
    "m0001_admin_order_list",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE admin_order_list (
                 id TEXT PRIMARY KEY NOT NULL,
                 email TEXT NOT NULL,
                 total_cents INTEGER NOT NULL,
                 currency TEXT NOT NULL,
                 status TEXT NOT NULL,
                 tracking_number TEXT,
                 created_at INTEGER NOT NULL
             )",
            "DROP TABLE admin_order_list"
        ),
        (
            "CREATE INDEX idx_admin_order_list_recent
                 ON admin_order_list (created_at DESC, id DESC)",
            "DROP INDEX idx_admin_order_list_recent"
        )
    ]
);
