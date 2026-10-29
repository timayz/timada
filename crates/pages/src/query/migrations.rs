use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "pages";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Pages0001]
}

pub struct Pages0001;
sqlx_migrator::sqlite_migration!(
    Pages0001,
    "pages",
    "0001_pages",
    vec_box![],
    vec_box![(
        "CREATE TABLE pages (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            slug TEXT NOT NULL,
            title TEXT NOT NULL,
            published INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, id)
        )",
        "DROP TABLE pages"
    )]
);
