use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001Regions;

sqlite_migration!(
    M0001Regions,
    "region",
    "m0001_regions",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE region_list (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                currency TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE region_list"
        ),
        (
            "CREATE TABLE region_countries (
                country_code TEXT PRIMARY KEY,
                region_id TEXT NOT NULL,
                tax_rate_bps INTEGER NOT NULL
            )",
            "DROP TABLE region_countries"
        )
    ]
);
