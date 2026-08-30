use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001Auth;

sqlite_migration!(
    M0001Auth,
    "auth",
    "m0001_auth",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE auth_sessions (
                token TEXT PRIMARY KEY,
                subject_id TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('admin', 'customer')),
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )",
            "DROP TABLE auth_sessions"
        ),
        (
            "CREATE TABLE admin_users (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE admin_users"
        )
    ]
);
