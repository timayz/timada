use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001RefundList;

sqlite_migration!(
    M0001RefundList,
    "payment",
    "m0001_refund_list",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE payment_refund_list (
                refund_id TEXT PRIMARY KEY,
                payment_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                amount_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                reason TEXT NOT NULL,
                refunded_at INTEGER NOT NULL
            )",
            "DROP TABLE payment_refund_list"
        ),
        (
            "CREATE INDEX payment_refund_list_refunded_at
             ON payment_refund_list (refunded_at)",
            "DROP INDEX payment_refund_list_refunded_at"
        )
    ]
);

pub struct M0002Provider;

sqlite_migration!(
    M0002Provider,
    "payment",
    "m0002_provider",
    vec_box![M0001RefundList],
    vec_box![
        // Read model: every refund that was asked for, and what became of it.
        (
            "CREATE TABLE payment_refund_request_list (
                refund_id TEXT PRIMARY KEY,
                payment_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                amount_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                reason TEXT NOT NULL,
                status TEXT NOT NULL,
                failure TEXT,
                psp_refund_reference TEXT,
                requested_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            "DROP TABLE payment_refund_request_list"
        ),
        (
            "CREATE INDEX payment_refund_request_list_status
             ON payment_refund_request_list (status, requested_at)",
            "DROP INDEX payment_refund_request_list_status"
        ),
        // Write side: the provider's session for a payment being paid.
        (
            "CREATE TABLE payment_provider_session (
                payment_id TEXT PRIMARY KEY,
                session_reference TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE payment_provider_session"
        ),
        // Write side: the refunds to hand to the provider, one row per
        // `RefundRequested` event (a retried refund is a new row).
        (
            "CREATE TABLE payment_provider_refund (
                request_id TEXT PRIMARY KEY,
                refund_id TEXT NOT NULL,
                payment_id TEXT NOT NULL,
                amount_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                next_attempt_at INTEGER NOT NULL DEFAULT 0,
                claimed_by TEXT,
                claimed_until INTEGER,
                provider_reference TEXT,
                done_at INTEGER
            )",
            "DROP TABLE payment_provider_refund"
        ),
        (
            "CREATE INDEX payment_provider_refund_due
             ON payment_provider_refund (done_at, next_attempt_at)",
            "DROP INDEX payment_provider_refund_due"
        ),
        (
            "CREATE INDEX payment_provider_refund_reference
             ON payment_provider_refund (provider_reference)",
            "DROP INDEX payment_provider_refund_reference"
        )
    ]
);

pub struct M0003Disputes;

sqlite_migration!(
    M0003Disputes,
    "payment",
    "m0003_disputes",
    vec_box![M0002Provider],
    vec_box![
        // Read model: every dispute, and what became of it.
        (
            "CREATE TABLE payment_dispute_list (
                dispute_id TEXT PRIMARY KEY,
                payment_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                amount_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                reason TEXT NOT NULL,
                status TEXT NOT NULL,
                respond_by INTEGER,
                opened_at INTEGER NOT NULL,
                closed_at INTEGER
            )",
            "DROP TABLE payment_dispute_list"
        ),
        (
            "CREATE INDEX payment_dispute_list_status
             ON payment_dispute_list (status, respond_by)",
            "DROP INDEX payment_dispute_list_status"
        ),
        (
            "CREATE INDEX payment_dispute_list_order
             ON payment_dispute_list (order_id)",
            "DROP INDEX payment_dispute_list_order"
        ),
        // Which payment a provider's reference was captured for: a provider
        // reports a dispute against its own reference, not the shop's id.
        (
            "CREATE TABLE payment_capture_reference (
                psp_reference TEXT PRIMARY KEY,
                payment_id TEXT NOT NULL
            )",
            "DROP TABLE payment_capture_reference"
        )
    ]
);

/// Read-model and write-side migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001RefundList, M0002Provider, M0003Disputes]
}
