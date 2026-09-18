use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001AdminUser;
pub struct M0002AdminSession;

sqlite_migration!(
    M0001AdminUser,
    "admin",
    "m0001_admin_user",
    vec_box![],
    vec_box![(
        "CREATE TABLE admin_user (
            id TEXT PRIMARY KEY,
            email TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        "DROP TABLE admin_user"
    )]
);

sqlite_migration!(
    M0002AdminSession,
    "admin",
    "m0002_admin_session",
    vec_box![M0001AdminUser],
    vec_box![
        (
            "CREATE TABLE admin_session (
                token_hash BLOB PRIMARY KEY,
                admin_id TEXT NOT NULL REFERENCES admin_user (id) ON DELETE CASCADE,
                expires_at INTEGER NOT NULL
            )",
            "DROP TABLE admin_session"
        ),
        (
            "CREATE INDEX admin_session_admin ON admin_session (admin_id)",
            "DROP INDEX admin_session_admin"
        )
    ]
);

/// Admin users and sessions, to register alongside the contexts' migrations.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001AdminUser, M0002AdminSession]
}
