use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "support";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Support0001]
}

pub struct Support0001;
sqlx_migrator::sqlite_migration!(
    Support0001,
    "support",
    "0001_messages",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE support_messages (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                first_name TEXT NOT NULL,
                last_name TEXT NOT NULL,
                email TEXT NOT NULL,
                body TEXT NOT NULL,
                language TEXT,
                status TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                handled_at INTEGER,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE support_messages"
        ),
        (
            "CREATE INDEX support_messages_status ON support_messages
                (tenant_id, status, received_at DESC)",
            "DROP INDEX support_messages_status"
        ),
    ]
);
