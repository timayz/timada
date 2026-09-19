use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001Outbox;

sqlite_migration!(
    M0001Outbox,
    "mailer",
    "m0001_outbox",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE mailer_outbox (
                message_id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                sender TEXT NOT NULL,
                recipient TEXT NOT NULL,
                subject TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                sent_at INTEGER,
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            )",
            "DROP TABLE mailer_outbox"
        ),
        (
            "CREATE INDEX mailer_outbox_pending ON mailer_outbox (sent_at, created_at)",
            "DROP INDEX mailer_outbox_pending"
        )
    ]
);

pub struct M0002OutboxDelivery;

// An HTML alternative, a retry schedule, and a lease so that several delivery
// workers never send the same row.
sqlite_migration!(
    M0002OutboxDelivery,
    "mailer",
    "m0002_outbox_delivery",
    vec_box![M0001Outbox],
    vec_box![
        (
            "ALTER TABLE mailer_outbox ADD COLUMN html_body TEXT",
            "ALTER TABLE mailer_outbox DROP COLUMN html_body"
        ),
        (
            "ALTER TABLE mailer_outbox ADD COLUMN next_attempt_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE mailer_outbox DROP COLUMN next_attempt_at"
        ),
        (
            "ALTER TABLE mailer_outbox ADD COLUMN claimed_by TEXT",
            "ALTER TABLE mailer_outbox DROP COLUMN claimed_by"
        ),
        (
            "ALTER TABLE mailer_outbox ADD COLUMN claimed_until INTEGER",
            "ALTER TABLE mailer_outbox DROP COLUMN claimed_until"
        )
    ]
);

/// The outbox table, to register alongside evento's migrations.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001Outbox, M0002OutboxDelivery]
}
