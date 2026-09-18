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

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001PromotionRedemption]
}
