use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001Discounts;

sqlite_migration!(
    M0001Discounts,
    "promotion",
    "m0001_discounts",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE admin_discount_list (
                id TEXT PRIMARY KEY,
                code TEXT NOT NULL,
                kind TEXT NOT NULL,
                value INTEGER NOT NULL,
                currency TEXT,
                starts_at INTEGER NOT NULL,
                ends_at INTEGER,
                usage_limit INTEGER,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE admin_discount_list"
        ),
        (
            // Write-side state, not a read model: the atomic usage counter.
            "CREATE TABLE discount_redemptions (
                discount_id TEXT PRIMARY KEY,
                redeemed INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE discount_redemptions"
        )
    ]
);
