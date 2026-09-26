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

/// Read-model and configuration migrations for this context, to register
/// alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001Supplier, M0002SourcedProduct, M0003Rule, M0004Offer]
}
