use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "tenancy";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Tenancy0001]
}

/// The tenancy read models: tenants and the company each sells as, what the
/// administrator's search and list read, members and their history,
/// invitations, API keys with their scopes, companies and their owners, and
/// the AliExpress account of each company.
pub struct Tenancy0001;
sqlx_migrator::sqlite_migration!(
    Tenancy0001,
    "tenancy",
    "0001_read_models",
    vec_box![],
    vec_box![
        (
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
                series_code TEXT
            )",
            "DROP TABLE tenancy_tenants"
        ),
        // What the administrator's list of tenants reads on every page: an
        // index in its own order, ignoring case.
        (
            "CREATE INDEX tenancy_tenants_list ON tenancy_tenants (name COLLATE NOCASE, id)",
            "DROP INDEX tenancy_tenants_list"
        ),
        (
            "CREATE INDEX tenancy_tenants_company ON tenancy_tenants (company_id)",
            "DROP INDEX tenancy_tenants_company"
        ),
        // What the administrator's search finds a tenant by: the slug and the
        // name, cut in runs of three characters so a search matches anywhere
        // in a word, ignoring case and accents. The rowid is the tenant row's.
        (
            "CREATE VIRTUAL TABLE tenancy_tenants_fts USING fts5(
                slug, name, tokenize = 'trigram remove_diacritics 1'
            )",
            "DROP TABLE tenancy_tenants_fts"
        ),
        // How many tenants there are, kept by the subscription instead of
        // counted: one row, started at zero.
        (
            "CREATE TABLE tenancy_totals (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                tenants INTEGER NOT NULL
            )",
            "DROP TABLE tenancy_totals"
        ),
        (
            "INSERT INTO tenancy_totals (id, tenants) VALUES (1, 0)",
            "DELETE FROM tenancy_totals"
        ),
        (
            "CREATE TABLE tenancy_members (
                tenant_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                role TEXT NOT NULL,
                PRIMARY KEY (tenant_id, user_id)
            )",
            "DROP TABLE tenancy_members"
        ),
        (
            "CREATE INDEX tenancy_members_user ON tenancy_members (user_id)",
            "DROP INDEX tenancy_members_user"
        ),
        (
            "CREATE TABLE tenancy_invitations (
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
            )",
            "DROP TABLE tenancy_invitations"
        ),
        (
            "CREATE INDEX tenancy_invitations_tenant ON tenancy_invitations (tenant_id)",
            "DROP INDEX tenancy_invitations_tenant"
        ),
        // What the invitation limit counts by: the invitations a tenant, or
        // an account, sent since a moment.
        (
            "CREATE INDEX tenancy_invitations_tenant_sent ON tenancy_invitations (tenant_id, sent_at)",
            "DROP INDEX tenancy_invitations_tenant_sent"
        ),
        (
            "CREATE INDEX tenancy_invitations_invited_by_sent ON tenancy_invitations (invited_by, sent_at)",
            "DROP INDEX tenancy_invitations_invited_by_sent"
        ),
        // The keys of a tenant's store API, with the scopes each one holds.
        (
            "CREATE TABLE tenancy_api_keys (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                label TEXT NOT NULL,
                hint TEXT NOT NULL,
                created_by TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                revoked_by TEXT,
                revoked_at INTEGER,
                scopes TEXT NOT NULL
            )",
            "DROP TABLE tenancy_api_keys"
        ),
        (
            "CREATE INDEX tenancy_api_keys_tenant ON tenancy_api_keys (tenant_id)",
            "DROP INDEX tenancy_api_keys_tenant"
        ),
        // Companies and their owners.
        (
            "CREATE TABLE tenancy_companies (
                id TEXT PRIMARY KEY,
                siren TEXT NOT NULL,
                legal_name TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE tenancy_companies"
        ),
        (
            "CREATE TABLE tenancy_company_owners (
                company_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                PRIMARY KEY (company_id, user_id)
            )",
            "DROP TABLE tenancy_company_owners"
        ),
        (
            "CREATE INDEX tenancy_company_owners_user ON tenancy_company_owners (user_id)",
            "DROP INDEX tenancy_company_owners_user"
        ),
        // The members history of a tenant: one row per event that changed who
        // belongs to it or who was invited, with the account that did it.
        (
            "CREATE TABLE tenancy_member_audit (
                seq INTEGER PRIMARY KEY,
                event_id TEXT NOT NULL UNIQUE,
                tenant_id TEXT NOT NULL,
                at INTEGER NOT NULL,
                action TEXT NOT NULL,
                actor TEXT NOT NULL,
                user_id TEXT,
                email TEXT,
                role TEXT
            )",
            "DROP TABLE tenancy_member_audit"
        ),
        (
            "CREATE INDEX tenancy_member_audit_tenant_seq
                ON tenancy_member_audit (tenant_id, seq)",
            "DROP INDEX tenancy_member_audit_tenant_seq"
        ),
        // The companies with an AliExpress account connected, and until when
        // its access lasts.
        (
            "CREATE TABLE tenancy_aliexpress_accounts (
                company_id TEXT PRIMARY KEY,
                expires_at INTEGER NOT NULL,
                refresh_expires_at INTEGER NOT NULL
            )",
            "DROP TABLE tenancy_aliexpress_accounts"
        ),
        (
            "CREATE INDEX tenancy_aliexpress_accounts_expires
                ON tenancy_aliexpress_accounts (expires_at)",
            "DROP INDEX tenancy_aliexpress_accounts_expires"
        ),
    ]
);
