use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "tenancy";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![
        ReadModels0001,
        Invitations0002,
        ApiKeys0003,
        TenantSearch0004,
        TenantList0005,
        ApiKeyScopes0006
    ]
}

pub struct ReadModels0001;
sqlx_migrator::sqlite_migration!(
    ReadModels0001,
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
                group_id INTEGER NOT NULL,
                member_count INTEGER NOT NULL
            )",
            "DROP TABLE tenancy_tenants"
        ),
        (
            "CREATE INDEX tenancy_tenants_group ON tenancy_tenants (group_id)",
            "DROP INDEX tenancy_tenants_group"
        ),
        (
            "CREATE INDEX tenancy_tenants_name ON tenancy_tenants (name)",
            "DROP INDEX tenancy_tenants_name"
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
    ]
);

pub struct Invitations0002;
sqlx_migrator::sqlite_migration!(
    Invitations0002,
    "tenancy",
    "0002_invitations",
    vec_box![("tenancy", "0001_read_models")],
    vec_box![
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
    ]
);

pub struct ApiKeys0003;
sqlx_migrator::sqlite_migration!(
    ApiKeys0003,
    "tenancy",
    "0003_api_keys",
    vec_box![("tenancy", "0002_invitations")],
    vec_box![
        (
            "CREATE TABLE tenancy_api_keys (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                label TEXT NOT NULL,
                hint TEXT NOT NULL,
                created_by TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                revoked_by TEXT,
                revoked_at INTEGER
            )",
            "DROP TABLE tenancy_api_keys"
        ),
        (
            "CREATE INDEX tenancy_api_keys_tenant ON tenancy_api_keys (tenant_id)",
            "DROP INDEX tenancy_api_keys_tenant"
        ),
    ]
);

/// What the administrator's search finds a tenant by: the slug and the name,
/// cut in runs of three characters so a search matches anywhere in a word,
/// ignoring case and accents. The rowid is the tenant row's.
pub struct TenantSearch0004;
sqlx_migrator::sqlite_migration!(
    TenantSearch0004,
    "tenancy",
    "0004_tenant_search",
    vec_box![("tenancy", "0003_api_keys")],
    vec_box![
        (
            "CREATE VIRTUAL TABLE tenancy_tenants_fts USING fts5(
                slug, name, tokenize = 'trigram remove_diacritics 1'
            )",
            "DROP TABLE tenancy_tenants_fts"
        ),
        (
            "INSERT INTO tenancy_tenants_fts (rowid, slug, name)
                SELECT rowid, slug, name FROM tenancy_tenants",
            "DELETE FROM tenancy_tenants_fts"
        ),
    ]
);

/// What the administrator's list of tenants reads on every page: an index in
/// its own order (the old one compared case-sensitively, so the list sorted
/// every tenant to show one page), and how many tenants there are, kept by
/// the subscription instead of counted.
pub struct TenantList0005;
sqlx_migrator::sqlite_migration!(
    TenantList0005,
    "tenancy",
    "0005_tenant_list",
    vec_box![("tenancy", "0004_tenant_search")],
    vec_box![
        (
            "DROP INDEX tenancy_tenants_name",
            "CREATE INDEX tenancy_tenants_name ON tenancy_tenants (name)"
        ),
        (
            "CREATE INDEX tenancy_tenants_list ON tenancy_tenants (name COLLATE NOCASE, id)",
            "DROP INDEX tenancy_tenants_list"
        ),
        (
            "CREATE TABLE tenancy_totals (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                tenants INTEGER NOT NULL
            )",
            "DROP TABLE tenancy_totals"
        ),
        (
            "INSERT INTO tenancy_totals (id, tenants) SELECT 1, count(*) FROM tenancy_tenants",
            "DELETE FROM tenancy_totals"
        ),
    ]
);

/// The scopes of each key. A key created before scopes existed has no
/// `Scoped` event and holds the legacy full access, which is the default.
pub struct ApiKeyScopes0006;
sqlx_migrator::sqlite_migration!(
    ApiKeyScopes0006,
    "tenancy",
    "0006_api_key_scopes",
    vec_box![("tenancy", "0005_tenant_list")],
    vec_box![(
        "ALTER TABLE tenancy_api_keys
            ADD COLUMN scopes TEXT NOT NULL DEFAULT 'store:read store:write'",
        "ALTER TABLE tenancy_api_keys DROP COLUMN scopes"
    )]
);
