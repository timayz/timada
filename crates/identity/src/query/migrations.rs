//! Schema of the read models, as sqlx_migrator migrations the composition
//! root appends after evento's own.

use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "identity";

/// Every migration of this crate.
pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Identity0001]
}

/// `identity_users` and `identity_sessions`, one row per aggregate, rewritten
/// in full by the subscription whenever the aggregate changes, with what the
/// administrator's search and list of users read.
pub struct Identity0001;

sqlx_migrator::sqlite_migration!(
    Identity0001,
    "identity",
    "0001_read_models",
    vec_box![],
    vec_box![
        // A user: the account, the name shown, the e-mail + password method
        // (the address and whether one is set), and the sign-in counters.
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
                sessions_revoked_at_ms INTEGER,
                email TEXT,
                has_password INTEGER NOT NULL
            )",
            "DROP TABLE identity_users"
        ),
        (
            "CREATE INDEX identity_users_account ON identity_users (account)",
            "DROP INDEX identity_users_account"
        ),
        // What the administrator's list of users reads on every page: an
        // index in its own order, ignoring case.
        (
            "CREATE INDEX identity_users_list
                ON identity_users (deleted, label COLLATE NOCASE, id)",
            "DROP INDEX identity_users_list"
        ),
        // What the administrator's search finds a user by: the account and
        // the name, cut in runs of three characters so a search matches
        // anywhere in a word, ignoring case and accents. The rowid is the
        // user row's.
        (
            "CREATE VIRTUAL TABLE identity_users_fts USING fts5(
                account, label, tokenize = 'trigram remove_diacritics 1'
            )",
            "DROP TABLE identity_users_fts"
        ),
        // How many users there are, kept by the subscription instead of
        // counted: one row, started at zero.
        (
            "CREATE TABLE identity_totals (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                users INTEGER NOT NULL
            )",
            "DROP TABLE identity_totals"
        ),
        (
            "INSERT INTO identity_totals (id, users) VALUES (1, 0)",
            "DELETE FROM identity_totals"
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
