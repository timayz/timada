use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "newsletter";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Newsletter0001, Newsletter0002]
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

pub struct Newsletter0002;
sqlx_migrator::sqlite_migration!(
    Newsletter0002,
    "newsletter",
    "0002_campaigns",
    vec_box![Newsletter0001],
    vec_box![
        (
            "CREATE TABLE newsletter_campaigns (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                subject TEXT NOT NULL,
                language TEXT,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                sent_at INTEGER,
                finished_at INTEGER,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE newsletter_campaigns"
        ),
        (
            "CREATE INDEX newsletter_campaigns_status ON newsletter_campaigns (status)",
            "DROP INDEX newsletter_campaigns_status"
        ),
        (
            "CREATE TABLE newsletter_deliveries (
                tenant_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                subscriber_id TEXT NOT NULL,
                failed INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, campaign_id, subscriber_id)
            )",
            "DROP TABLE newsletter_deliveries"
        ),
    ]
);
