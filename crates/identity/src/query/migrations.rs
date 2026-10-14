//! Schema of the read models, as sqlx_migrator migrations the composition
//! root appends after evento's own.

use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "identity";

/// Every migration of this crate, oldest first.
pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![ReadModels0001, Email0002, UserSearch0003, UserList0004]
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

/// What the administrator's search finds a user by: the account and the
/// name, cut in runs of three characters so a search matches anywhere in a
/// word, ignoring case and accents. The rowid is the user row's.
pub struct UserSearch0003;
sqlx_migrator::sqlite_migration!(
    UserSearch0003,
    "identity",
    "0003_user_search",
    vec_box![("identity", "0002_email")],
    vec_box![
        (
            "CREATE VIRTUAL TABLE identity_users_fts USING fts5(
                account, label, tokenize = 'trigram remove_diacritics 1'
            )",
            "DROP TABLE identity_users_fts"
        ),
        (
            "INSERT INTO identity_users_fts (rowid, account, label)
                SELECT rowid, account, label FROM identity_users",
            "DELETE FROM identity_users_fts"
        ),
    ]
);

/// What the administrator's list of users reads on every page: an index in
/// its own order (the old one compared case-sensitively, so the list sorted
/// every user to show one page), and how many users there are, kept by the
/// subscription instead of counted.
pub struct UserList0004;
sqlx_migrator::sqlite_migration!(
    UserList0004,
    "identity",
    "0004_user_list",
    vec_box![("identity", "0003_user_search")],
    vec_box![
        (
            "DROP INDEX identity_users_label",
            "CREATE INDEX identity_users_label ON identity_users (deleted, label)"
        ),
        (
            "CREATE INDEX identity_users_list
                ON identity_users (deleted, label COLLATE NOCASE, id)",
            "DROP INDEX identity_users_list"
        ),
        (
            "CREATE TABLE identity_totals (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                users INTEGER NOT NULL
            )",
            "DROP TABLE identity_totals"
        ),
        (
            "INSERT INTO identity_totals (id, users) SELECT 1, count(*) FROM identity_users",
            "DELETE FROM identity_totals"
        ),
    ]
);
