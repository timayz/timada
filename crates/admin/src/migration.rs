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

pub struct M0003AdminRole;

sqlite_migration!(
    M0003AdminRole,
    "admin",
    "m0003_admin_role",
    vec_box![M0002AdminSession],
    vec_box![(
        // Whoever signed in before there were roles could do everything:
        // they stay the shop's owners.
        "ALTER TABLE admin_user ADD COLUMN role TEXT NOT NULL DEFAULT 'owner'",
        "ALTER TABLE admin_user DROP COLUMN role"
    )]
);

pub struct M0004AdminTeam;

sqlite_migration!(
    M0004AdminTeam,
    "admin",
    "m0004_admin_team",
    vec_box![M0003AdminRole],
    vec_box![
        // Who left no longer signs in; nothing is deleted.
        (
            "ALTER TABLE admin_user ADD COLUMN active INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE admin_user DROP COLUMN active"
        ),
        // Given a temporary password, to be replaced at the first sign-in.
        (
            "ALTER TABLE admin_user ADD COLUMN must_change_password INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE admin_user DROP COLUMN must_change_password"
        )
    ]
);

/// Admin users and sessions, to register alongside the contexts' migrations.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![
        M0001AdminUser,
        M0002AdminSession,
        M0003AdminRole,
        M0004AdminTeam
    ]
}
