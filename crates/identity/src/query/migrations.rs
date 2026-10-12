//! Schema of the read models, as sqlx_migrator migrations the composition
//! root appends after evento's own.

use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "identity";

/// Every migration of this crate, oldest first.
pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![ReadModels0001, Email0002]
}

/// `identity_users` and `identity_sessions`, one row per aggregate, rewritten
/// in full by the subscription whenever the aggregate changes.
pub struct ReadModels0001;

sqlx_migrator::sqlite_migration!(
    ReadModels0001,
    "identity",
    "0001_read_models",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE identity_users (
                id TEXT PRIMARY KEY,
                account TEXT NOT NULL,
                label TEXT NOT NULL,
                is_admin INTEGER NOT NULL,
                deleted INTEGER NOT NULL,
                registered_at INTEGER NOT NULL,
                last_sign_in_at INTEGER,
                sign_in_count INTEGER NOT NULL,
                oauth2_count INTEGER NOT NULL,
                passkey_count INTEGER NOT NULL,
                sessions_revoked_at_ms INTEGER
            )",
            "DROP TABLE identity_users"
        ),
        (
            "CREATE INDEX identity_users_label ON identity_users (deleted, label)",
            "DROP INDEX identity_users_label"
        ),
        (
            "CREATE INDEX identity_users_account ON identity_users (account)",
            "DROP INDEX identity_users_account"
        ),
        (
            "CREATE TABLE identity_sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                started_at_ms INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                ended INTEGER NOT NULL,
                user_agent TEXT,
                ip TEXT
            )",
            "DROP TABLE identity_sessions"
        ),
        (
            "CREATE INDEX identity_sessions_user ON identity_sessions (user_id, ended, expires_at)",
            "DROP INDEX identity_sessions_user"
        ),
    ]
);

/// The e-mail + password method: the address and whether one is set.
pub struct Email0002;

sqlx_migrator::sqlite_migration!(
    Email0002,
    "identity",
    "0002_email",
    vec_box![("identity", "0001_read_models")],
    vec_box![
        (
            "ALTER TABLE identity_users ADD COLUMN email TEXT",
            "ALTER TABLE identity_users DROP COLUMN email"
        ),
        (
            "ALTER TABLE identity_users ADD COLUMN has_password INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE identity_users DROP COLUMN has_password"
        ),
    ]
);
