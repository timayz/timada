use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "newsletter";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Newsletter0001]
}

pub struct Newsletter0001;
sqlx_migrator::sqlite_migration!(
    Newsletter0001,
    "newsletter",
    "0001_subscribers",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE newsletter_subscribers (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                email TEXT NOT NULL,
                language TEXT,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                requested_at INTEGER NOT NULL,
                confirmed_at INTEGER,
                unsubscribed_at INTEGER,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE newsletter_subscribers"
        ),
        (
            "CREATE INDEX newsletter_subscribers_status ON newsletter_subscribers
                (tenant_id, status, requested_at DESC)",
            "DROP INDEX newsletter_subscribers_status"
        ),
    ]
);
