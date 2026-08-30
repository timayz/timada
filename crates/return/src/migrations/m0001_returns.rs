use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001Returns;

sqlite_migration!(
    M0001Returns,
    "return",
    "m0001_returns",
    vec_box![],
    vec_box![(
        "CREATE TABLE admin_return_list (
            id TEXT PRIMARY KEY,
            order_id TEXT NOT NULL,
            status TEXT NOT NULL,
            reason TEXT NOT NULL,
            reject_reason TEXT,
            requested_at INTEGER NOT NULL
        )",
        "DROP TABLE admin_return_list"
    )]
);
