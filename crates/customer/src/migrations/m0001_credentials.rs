use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001Credentials;

sqlite_migration!(
    M0001Credentials,
    "customer",
    "m0001_credentials",
    vec_box![],
    vec_box![(
        "CREATE TABLE customer_credentials (
            customer_id TEXT PRIMARY KEY,
            email TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        "DROP TABLE customer_credentials"
    )]
);
