//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "identity";

/// `identity_users` and `identity_sessions`, one row per aggregate, rewritten
/// in full by the subscription whenever the aggregate changes, with what the
/// administrator's search and list of users read.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_read_models",
        // A user: the account, the name shown, the e-mail + password method
        // (the address and whether one is set), and the sign-in counters.
        // `account_search` and `label_search` are what the administrator's
        // search finds a user by: the account and the name lowercased and
        // stripped of accents, under an index cut in runs of three characters
        // so a search matches anywhere in a word.
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
            has_password INTEGER NOT NULL,
            account_search TEXT NOT NULL,
            label_search TEXT NOT NULL
        );
        CREATE INDEX identity_users_account ON identity_users (account);
        CREATE INDEX identity_users_list
            ON identity_users (deleted, label COLLATE NOCASE, id);
        CREATE INDEX identity_users_fts ON identity_users
            USING fts(account_search, label_search)
            WITH (tokenizer = 'ngram', min_gram = 3, max_gram = 3);
        CREATE TABLE identity_totals (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            users INTEGER NOT NULL
        );
        INSERT INTO identity_totals (id, users) VALUES (1, 0);
        CREATE TABLE identity_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            started_at_ms INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            ended INTEGER NOT NULL,
            user_agent TEXT,
            ip TEXT
        );
        CREATE INDEX identity_sessions_user ON identity_sessions (user_id, ended, expires_at);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &["identity_users", "identity_totals", "identity_sessions"];

/// What [`TABLES`] hold once emptied that the schema expects: the totals
/// row. Run after emptying them.
pub const RESET_ROWS: &str = "INSERT INTO identity_totals (id, users) VALUES (1, 0)";
