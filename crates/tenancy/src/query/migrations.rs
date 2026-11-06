//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "tenancy";

/// The tenancy read models: tenants and the company each sells as, what the
/// administrator's search and list read, members and their history,
/// invitations, API keys with their scopes, companies and their owners, and
/// the AliExpress account of each company.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_read_models",
        // `slug_search` and `name_search` are what the administrator's
        // search finds a tenant by: the slug and the name lowercased and
        // stripped of accents, under an index cut in runs of three
        // characters so a search matches anywhere in a word.
        "CREATE TABLE tenancy_tenants (
            id TEXT PRIMARY KEY,
            slug TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            created_by TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            suspended INTEGER NOT NULL,
            suspended_reason TEXT,
            member_count INTEGER NOT NULL,
            company_id TEXT,
            series_code TEXT,
            slug_search TEXT NOT NULL,
            name_search TEXT NOT NULL
        );
        CREATE INDEX tenancy_tenants_list ON tenancy_tenants (name COLLATE NOCASE, id);
        CREATE INDEX tenancy_tenants_company ON tenancy_tenants (company_id);
        CREATE INDEX tenancy_tenants_fts ON tenancy_tenants
            USING fts(slug_search, name_search)
            WITH (tokenizer = 'ngram', min_gram = 3, max_gram = 3);
        CREATE TABLE tenancy_totals (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            tenants INTEGER NOT NULL
        );
        INSERT INTO tenancy_totals (id, tenants) VALUES (1, 0);
        CREATE TABLE tenancy_members (
            tenant_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            role TEXT NOT NULL,
            PRIMARY KEY (tenant_id, user_id)
        );
        CREATE INDEX tenancy_members_user ON tenancy_members (user_id);
        CREATE TABLE tenancy_invitations (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            email TEXT NOT NULL,
            role TEXT NOT NULL,
            invited_by TEXT NOT NULL,
            sent_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            accepted_by TEXT,
            accepted_at INTEGER,
            revoked INTEGER NOT NULL
        );
        CREATE INDEX tenancy_invitations_tenant ON tenancy_invitations (tenant_id);
        CREATE INDEX tenancy_invitations_tenant_sent ON tenancy_invitations (tenant_id, sent_at);
        CREATE INDEX tenancy_invitations_invited_by_sent
            ON tenancy_invitations (invited_by, sent_at);
        CREATE TABLE tenancy_api_keys (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            label TEXT NOT NULL,
            hint TEXT NOT NULL,
            created_by TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            revoked_by TEXT,
            revoked_at INTEGER,
            scopes TEXT NOT NULL
        );
        CREATE INDEX tenancy_api_keys_tenant ON tenancy_api_keys (tenant_id);
        CREATE TABLE tenancy_companies (
            id TEXT PRIMARY KEY,
            siren TEXT NOT NULL,
            legal_name TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE tenancy_company_owners (
            company_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            PRIMARY KEY (company_id, user_id)
        );
        CREATE INDEX tenancy_company_owners_user ON tenancy_company_owners (user_id);
        CREATE TABLE tenancy_member_audit (
            seq INTEGER PRIMARY KEY,
            event_id TEXT NOT NULL UNIQUE,
            tenant_id TEXT NOT NULL,
            at INTEGER NOT NULL,
            action TEXT NOT NULL,
            actor TEXT NOT NULL,
            user_id TEXT,
            email TEXT,
            role TEXT
        );
        CREATE INDEX tenancy_member_audit_tenant_seq ON tenancy_member_audit (tenant_id, seq);
        CREATE TABLE tenancy_aliexpress_accounts (
            company_id TEXT PRIMARY KEY,
            expires_at INTEGER NOT NULL,
            refresh_expires_at INTEGER NOT NULL
        );
        CREATE INDEX tenancy_aliexpress_accounts_expires
            ON tenancy_aliexpress_accounts (expires_at);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &[
    "tenancy_tenants",
    "tenancy_totals",
    "tenancy_members",
    "tenancy_invitations",
    "tenancy_api_keys",
    "tenancy_companies",
    "tenancy_company_owners",
    "tenancy_member_audit",
    "tenancy_aliexpress_accounts",
];

/// What [`TABLES`] hold once emptied that the schema expects: the totals
/// row. Run after emptying them.
pub const RESET_ROWS: &str = "INSERT INTO tenancy_totals (id, tenants) VALUES (1, 0)";
