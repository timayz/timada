use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001PromotionRedemption;

sqlite_migration!(
    M0001PromotionRedemption,
    "promotion",
    "m0001_promotion_redemption",
    vec_box![],
    vec_box![(
        "CREATE TABLE promotion_redemption (
            code TEXT NOT NULL,
            order_id TEXT NOT NULL,
            PRIMARY KEY (code, order_id)
        )",
        "DROP TABLE promotion_redemption"
    )]
);

pub struct M0002PromotionCodeList;

sqlite_migration!(
    M0002PromotionCodeList,
    "promotion",
    "m0002_promotion_code_list",
    vec_box![M0001PromotionRedemption],
    vec_box![
        (
            "CREATE TABLE promotion_code_list (
                id TEXT PRIMARY KEY,
                code TEXT NOT NULL,
                kind TEXT NOT NULL,
                percent_bp INTEGER,
                amount_minor INTEGER,
                currency TEXT,
                active INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE promotion_code_list"
        ),
        (
            "CREATE INDEX promotion_code_list_kind ON promotion_code_list (kind, created_at)",
            "DROP INDEX promotion_code_list_kind"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001PromotionRedemption, M0002PromotionCodeList]
}
