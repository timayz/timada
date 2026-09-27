use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001Supplier;

sqlite_migration!(
    M0001Supplier,
    "sourcing",
    "m0001_supplier",
    vec_box![],
    vec_box![(
        "CREATE TABLE sourcing_supplier (
            supplier_id TEXT PRIMARY KEY,
            slug TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            connector TEXT NOT NULL,
            currency TEXT NOT NULL,
            suspended INTEGER NOT NULL DEFAULT 0,
            suspended_reason TEXT,
            registered_at INTEGER NOT NULL
        )",
        "DROP TABLE sourcing_supplier"
    )]
);

pub struct M0002SourcedProduct;

sqlite_migration!(
    M0002SourcedProduct,
    "sourcing",
    "m0002_sourced_product",
    vec_box![M0001Supplier],
    vec_box![
        (
            "CREATE TABLE sourcing_product (
                product_id TEXT PRIMARY KEY,
                sourced_id TEXT NOT NULL,
                supplier_id TEXT NOT NULL,
                external_item_id TEXT NOT NULL,
                external_sku TEXT,
                locked INTEGER NOT NULL DEFAULT 0,
                locked_reason TEXT,
                active INTEGER NOT NULL DEFAULT 1,
                sourced_at INTEGER NOT NULL
            )",
            "DROP TABLE sourcing_product"
        ),
        (
            "CREATE INDEX sourcing_product_supplier
             ON sourcing_product (supplier_id, active)",
            "DROP INDEX sourcing_product_supplier"
        )
    ]
);

pub struct M0003Rule;

// Configuration, not a fact: a markup that could never be tuned again would
// be the one shape in this workspace nobody wanted frozen.
sqlite_migration!(
    M0003Rule,
    "sourcing",
    "m0003_rule",
    vec_box![M0002SourcedProduct],
    vec_box![(
        "CREATE TABLE sourcing_rule (
            scope TEXT PRIMARY KEY,
            markup_bp INTEGER NOT NULL,
            min_margin_bp INTEGER NOT NULL,
            round_step_minor INTEGER NOT NULL,
            round_ends_minor INTEGER NOT NULL,
            auto_move_bp INTEGER NOT NULL,
            auto_move_cap_minor INTEGER NOT NULL,
            auto_move_cap_currency TEXT NOT NULL,
            shipping_included INTEGER NOT NULL,
            eco_on_top INTEGER NOT NULL,
            safety_stock INTEGER NOT NULL
        )",
        "DROP TABLE sourcing_rule"
    )]
);

pub struct M0004Offer;

// The last word of each supplier about each item. Operational data, kept
// where it can be overwritten: polling a catalogue of twenty thousand every
// few hours would otherwise append events by the hundred thousand a day and
// tell posterity nothing.
sqlite_migration!(
    M0004Offer,
    "sourcing",
    "m0004_offer",
    vec_box![M0003Rule],
    vec_box![(
        "CREATE TABLE sourcing_offer (
            product_id TEXT PRIMARY KEY,
            supplier_id TEXT NOT NULL,
            cost_minor INTEGER NOT NULL,
            cost_currency TEXT NOT NULL,
            shipping_minor INTEGER NOT NULL,
            available INTEGER NOT NULL,
            title TEXT,
            url TEXT,
            landed_minor INTEGER,
            landed_currency TEXT,
            rate_micros INTEGER,
            rate_source TEXT,
            rate_as_of INTEGER,
            fetched_at INTEGER NOT NULL
        )",
        "DROP TABLE sourcing_offer"
    )]
);

pub struct M0005Poll;

// Which products are due to be asked about, and when. Claimed rows with a
// lease, so any number of workers may run — the mailer outbox's shape.
sqlite_migration!(
    M0005Poll,
    "sourcing",
    "m0005_poll",
    vec_box![M0004Offer],
    vec_box![
        (
            "CREATE TABLE sourcing_poll (
                product_id TEXT PRIMARY KEY,
                supplier_id TEXT NOT NULL,
                next_poll_at INTEGER NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                claimed_by TEXT,
                claimed_until INTEGER,
                last_error TEXT,
                last_polled_at INTEGER
            )",
            "DROP TABLE sourcing_poll"
        ),
        (
            "CREATE INDEX sourcing_poll_due ON sourcing_poll (next_poll_at)",
            "DROP INDEX sourcing_poll_due"
        )
    ]
);

pub struct M0006PriceReview;

// What the guardrails would not let through by itself. The id derives from
// the product and the reason, so a product polled forty times before anybody
// looks is one row, refreshed — not forty.
sqlite_migration!(
    M0006PriceReview,
    "sourcing",
    "m0006_price_review",
    vec_box![M0005Poll],
    vec_box![
        (
            "CREATE TABLE sourcing_price_review (
                review_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                supplier_id TEXT NOT NULL,
                reason TEXT NOT NULL,
                current_minor INTEGER,
                proposed_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                cost_minor INTEGER NOT NULL,
                cost_currency TEXT NOT NULL,
                margin_bp INTEGER NOT NULL,
                raised_at INTEGER NOT NULL,
                settled_at INTEGER,
                settled_as TEXT
            )",
            "DROP TABLE sourcing_price_review"
        ),
        (
            "CREATE INDEX sourcing_price_review_open
             ON sourcing_price_review (settled_at, raised_at)",
            "DROP INDEX sourcing_price_review_open"
        )
    ]
);

pub struct M0007Purchase;

sqlite_migration!(
    M0007Purchase,
    "sourcing",
    "m0007_purchase",
    vec_box![M0006PriceReview],
    vec_box![
        (
            "CREATE TABLE sourcing_purchase (
                purchase_id TEXT PRIMARY KEY,
                order_id TEXT NOT NULL,
                supplier_id TEXT NOT NULL,
                status TEXT NOT NULL,
                external_order_id TEXT,
                cost_minor INTEGER NOT NULL,
                cost_currency TEXT NOT NULL,
                charged_minor INTEGER,
                units INTEGER NOT NULL,
                carrier TEXT,
                tracking_number TEXT,
                note TEXT,
                drafted_at INTEGER NOT NULL,
                settled_at INTEGER
            )",
            "DROP TABLE sourcing_purchase"
        ),
        (
            "CREATE INDEX sourcing_purchase_status ON sourcing_purchase (status, drafted_at)",
            "DROP INDEX sourcing_purchase_status"
        ),
        (
            "CREATE INDEX sourcing_purchase_order ON sourcing_purchase (order_id)",
            "DROP INDEX sourcing_purchase_order"
        )
    ]
);

pub struct M0008PurchaseWork;

// Placing, tracking and calling off, one row per thing to say to a supplier.
// The row id carries the purchase, so it is also the connector's idempotency
// key: a worker that died after the supplier answered buys nothing twice.
sqlite_migration!(
    M0008PurchaseWork,
    "sourcing",
    "m0008_purchase_work",
    vec_box![M0007Purchase],
    vec_box![
        (
            "CREATE TABLE sourcing_purchase_work (
                work_id TEXT PRIMARY KEY,
                purchase_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                next_attempt_at INTEGER NOT NULL DEFAULT 0,
                attempts INTEGER NOT NULL DEFAULT 0,
                claimed_by TEXT,
                claimed_until INTEGER,
                last_error TEXT,
                done_at INTEGER
            )",
            "DROP TABLE sourcing_purchase_work"
        ),
        (
            "CREATE INDEX sourcing_purchase_work_due
             ON sourcing_purchase_work (done_at, next_attempt_at)",
            "DROP INDEX sourcing_purchase_work_due"
        )
    ]
);

/// Read-model and configuration migrations for this context, to register
/// alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![
        M0001Supplier,
        M0002SourcedProduct,
        M0003Rule,
        M0004Offer,
        M0005Poll,
        M0006PriceReview,
        M0007Purchase,
        M0008PurchaseWork
    ]
}
