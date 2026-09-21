use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001CustomerList;

sqlite_migration!(
    M0001CustomerList,
    "customer",
    "m0001_customer_list",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE customer_list (
                customer_id TEXT PRIMARY KEY,
                email TEXT NOT NULL,
                first_name TEXT NOT NULL,
                last_name TEXT NOT NULL,
                registered_at INTEGER NOT NULL
            )",
            "DROP TABLE customer_list"
        ),
        (
            "CREATE INDEX customer_list_email ON customer_list (email)",
            "DROP INDEX customer_list_email"
        )
    ]
);

pub struct M0002CustomerListGuest;

sqlite_migration!(
    M0002CustomerListGuest,
    "customer",
    "m0002_customer_list_guest",
    vec_box![M0001CustomerList],
    vec_box![(
        // Ordered without an account, and has none so far.
        "ALTER TABLE customer_list ADD COLUMN guest INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE customer_list DROP COLUMN guest"
    )]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001CustomerList, M0002CustomerListGuest]
}
