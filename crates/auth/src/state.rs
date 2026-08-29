use sqlx::SqlitePool;

/// Everything the auth routes and middleware need. Auth is not event-sourced,
/// so this is the one state struct in the workspace holding bare pools instead
/// of a `ServiceContext`.
#[derive(Clone)]
pub struct AuthState {
    pub read_pool: SqlitePool,
    pub write_pool: SqlitePool,
}
