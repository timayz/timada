use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "newsletter";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![
        Newsletter0001,
        Newsletter0002,
        Newsletter0003,
        Newsletter0004
    ]
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

pub struct Newsletter0003;
sqlx_migrator::sqlite_migration!(
    Newsletter0003,
    "newsletter",
    "0003_subscriber_time_zones",
    vec_box![Newsletter0002],
    vec_box![(
        "ALTER TABLE newsletter_subscribers ADD COLUMN time_zone TEXT",
        "ALTER TABLE newsletter_subscribers DROP COLUMN time_zone"
    )]
);

pub struct Newsletter0004;
sqlx_migrator::sqlite_migration!(
    Newsletter0004,
    "newsletter",
    "0004_schedules",
    vec_box![Newsletter0003],
    vec_box![
        (
            "ALTER TABLE newsletter_campaigns ADD COLUMN cadence TEXT",
            "ALTER TABLE newsletter_campaigns DROP COLUMN cadence"
        ),
        (
            "CREATE TABLE newsletter_editions (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                cycle TEXT NOT NULL,
                status TEXT NOT NULL,
                opened_at INTEGER NOT NULL,
                closed_at INTEGER,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE newsletter_editions"
        ),
        (
            "CREATE INDEX newsletter_editions_campaign ON newsletter_editions
                (tenant_id, campaign_id, opened_at DESC)",
            "DROP INDEX newsletter_editions_campaign"
        ),
        (
            "CREATE INDEX newsletter_editions_status ON newsletter_editions (status)",
            "DROP INDEX newsletter_editions_status"
        ),
        // A recurring campaign mails a subscriber once per edition: the
        // deliveries are keyed by edition (a campaign sent at once is its own).
        (
            "CREATE TABLE newsletter_deliveries_by_edition (
                tenant_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                edition_id TEXT NOT NULL,
                subscriber_id TEXT NOT NULL,
                failed INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, edition_id, subscriber_id)
            )",
            "DROP TABLE newsletter_deliveries_by_edition"
        ),
        (
            "INSERT INTO newsletter_deliveries_by_edition
                (tenant_id, campaign_id, edition_id, subscriber_id, failed)
             SELECT tenant_id, campaign_id, campaign_id, subscriber_id, failed
             FROM newsletter_deliveries",
            "INSERT INTO newsletter_deliveries
                (tenant_id, campaign_id, subscriber_id, failed)
             SELECT tenant_id, campaign_id, subscriber_id, MAX(failed)
             FROM newsletter_deliveries_by_edition
             GROUP BY tenant_id, campaign_id, subscriber_id"
        ),
        (
            "DROP TABLE newsletter_deliveries",
            "CREATE TABLE newsletter_deliveries (
                tenant_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                subscriber_id TEXT NOT NULL,
                failed INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, campaign_id, subscriber_id)
            )"
        ),
        (
            "ALTER TABLE newsletter_deliveries_by_edition RENAME TO newsletter_deliveries",
            "ALTER TABLE newsletter_deliveries RENAME TO newsletter_deliveries_by_edition"
        ),
        (
            "CREATE INDEX newsletter_deliveries_campaign ON newsletter_deliveries
                (tenant_id, campaign_id)",
            "DROP INDEX newsletter_deliveries_campaign"
        ),
    ]
);
