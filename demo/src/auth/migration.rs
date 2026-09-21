use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001ShopAccount;
pub struct M0002ShopSession;
pub struct M0003ShopPasswordReset;

// `customer_id` is NULL while a sign-up holds the email claim and has not
// registered its customer yet.
sqlite_migration!(
    M0001ShopAccount,
    "shop",
    "m0001_shop_account",
    vec_box![],
    vec_box![(
        "CREATE TABLE shop_account (
            email TEXT PRIMARY KEY,
            customer_id TEXT UNIQUE,
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        "DROP TABLE shop_account"
    )]
);

sqlite_migration!(
    M0002ShopSession,
    "shop",
    "m0002_shop_session",
    vec_box![M0001ShopAccount],
    vec_box![
        (
            "CREATE TABLE shop_session (
                token_hash BLOB PRIMARY KEY,
                customer_id TEXT NOT NULL REFERENCES shop_account (customer_id) ON DELETE CASCADE,
                expires_at INTEGER NOT NULL
            )",
            "DROP TABLE shop_session"
        ),
        (
            "CREATE INDEX shop_session_customer ON shop_session (customer_id)",
            "DROP INDEX shop_session_customer"
        )
    ]
);

// Only the hash of a reset link's token is kept, like a session's: reading
// the table gives nobody a way in.
sqlite_migration!(
    M0003ShopPasswordReset,
    "shop",
    "m0003_shop_password_reset",
    vec_box![M0002ShopSession],
    vec_box![
        (
            "CREATE TABLE shop_password_reset (
                token_hash BLOB PRIMARY KEY,
                customer_id TEXT NOT NULL REFERENCES shop_account (customer_id) ON DELETE CASCADE,
                requested_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )",
            "DROP TABLE shop_password_reset"
        ),
        (
            "CREATE INDEX shop_password_reset_customer ON shop_password_reset (customer_id)",
            "DROP INDEX shop_password_reset_customer"
        )
    ]
);

/// Shopper accounts and sessions, registered next to the contexts' migrations.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001ShopAccount, M0002ShopSession, M0003ShopPasswordReset]
}
