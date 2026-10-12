use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "tenancy";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![ReadModels0001]
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
