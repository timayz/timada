//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "newsletter";

/// The read models of this context, one table each (see the columns).
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_newsletter",
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
            );
        CREATE INDEX newsletter_subscribers_status ON newsletter_subscribers
                (tenant_id, status, requested_at DESC);
        CREATE TABLE newsletter_campaigns (
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
            );
        CREATE INDEX newsletter_campaigns_status ON newsletter_campaigns (status);
        CREATE TABLE newsletter_editions (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                cycle TEXT NOT NULL,
                status TEXT NOT NULL,
                opened_at INTEGER NOT NULL,
                closed_at INTEGER,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX newsletter_editions_campaign ON newsletter_editions
                (tenant_id, campaign_id, opened_at DESC);
        CREATE INDEX newsletter_editions_status ON newsletter_editions (status);
        CREATE TABLE newsletter_deliveries (
                tenant_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                edition_id TEXT NOT NULL,
                subscriber_id TEXT NOT NULL,
                failed INTEGER NOT NULL,
                skipped INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, edition_id, subscriber_id)
            );
        CREATE INDEX newsletter_deliveries_campaign ON newsletter_deliveries
                (tenant_id, campaign_id);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &[
    "newsletter_subscribers",
    "newsletter_campaigns",
    "newsletter_editions",
    "newsletter_deliveries",
];
