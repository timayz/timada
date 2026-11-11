//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "newsletter";

/// The read models of this context, one table each (see the columns).
///
/// The counts a page or the sending reads are kept as the rows change,
/// never counted over the deliveries or the subscribers: a campaign's and an
/// edition's mails by outcome, an edition's mails by time zone
/// (`newsletter_edition_zones`), and the confirmed subscribers by language
/// and time zone (`newsletter_audience`, `''` for none, since a key column
/// holding `NULL` never conflicts). A subscriber's `shard` is
/// [`timada_core::shard_of`] its id: which node of the cluster mails them.
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
                shard INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX newsletter_subscribers_status ON newsletter_subscribers
                (tenant_id, status, requested_at DESC);
        CREATE TABLE newsletter_audience (
                tenant_id TEXT NOT NULL,
                language TEXT NOT NULL,
                time_zone TEXT NOT NULL,
                confirmed INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, language, time_zone)
            );
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
                delivered INTEGER NOT NULL DEFAULT 0,
                failed INTEGER NOT NULL DEFAULT 0,
                skipped INTEGER NOT NULL DEFAULT 0,
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
                delivered INTEGER NOT NULL DEFAULT 0,
                failed INTEGER NOT NULL DEFAULT 0,
                skipped INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX newsletter_editions_campaign ON newsletter_editions
                (tenant_id, campaign_id, opened_at DESC);
        CREATE INDEX newsletter_editions_status ON newsletter_editions (status);
        CREATE TABLE newsletter_edition_zones (
                tenant_id TEXT NOT NULL,
                edition_id TEXT NOT NULL,
                time_zone TEXT NOT NULL,
                mailed INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, edition_id, time_zone)
            );
        CREATE TABLE newsletter_deliveries (
                tenant_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                edition_id TEXT NOT NULL,
                subscriber_id TEXT NOT NULL,
                failed INTEGER NOT NULL,
                skipped INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, edition_id, subscriber_id)
            );",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &[
    "newsletter_subscribers",
    "newsletter_audience",
    "newsletter_campaigns",
    "newsletter_editions",
    "newsletter_edition_zones",
    "newsletter_deliveries",
];
