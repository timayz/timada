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
    "0001_newsletter",
    vec_box![],
    vec_box![
        // Subscribers of a shop's newsletter; `time_zone` is the IANA zone a
        // paced edition is delivered in.
        (
            "CREATE TABLE newsletter_subscribers (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                email TEXT NOT NULL,
                language TEXT,
                time_zone TEXT,
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
        // Campaigns; `cadence` is NULL for one sent at once and names the
        // recurrence of a scheduled one.
        (
            "CREATE TABLE newsletter_campaigns (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                subject TEXT NOT NULL,
                language TEXT,
                cadence TEXT,
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
        // Each issue of a recurring campaign, one per cycle.
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
        // A subscriber is mailed once per edition, so deliveries are keyed by
        // edition (a campaign sent at once is its own edition).
        (
            "CREATE TABLE newsletter_deliveries (
                tenant_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                edition_id TEXT NOT NULL,
                subscriber_id TEXT NOT NULL,
                failed INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, edition_id, subscriber_id)
            )",
            "DROP TABLE newsletter_deliveries"
        ),
        (
            "CREATE INDEX newsletter_deliveries_campaign ON newsletter_deliveries
                (tenant_id, campaign_id)",
            "DROP INDEX newsletter_deliveries_campaign"
        ),
    ]
);
