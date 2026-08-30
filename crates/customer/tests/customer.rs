//! Registration and login against a real SQLite database and event store —
//! the credentials-row-then-event dance is exactly what production runs.

use axum_extra::extract::cookie::{Cookie, CookieJar};
use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_auth::{AuthState, SESSION_COOKIE};
use timada_core::{ServiceContext, new_id};
use timada_customer::{
    CustomerState, LoginError, RegisterError, current_customer, load_customer, login_customer,
    register_customer,
};

/// A temp-file database plus the state every test needs, torn down on close.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: CustomerState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-customer-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());

        // One pool for both roles: these tests are single-threaded CLI-shaped
        // work, and a read-only pool could not run the migrations.
        let pool = timada_core::db::create_pool(&url, 1).await?;

        let mut migrator = Migrator::<sqlx::Sqlite>::default();
        let mut migrations = timada_auth::migrations();
        migrations.extend(timada_customer::migrations());
        migrator.add_migrations(migrations)?;
        let mut conn = pool.acquire().await?;
        migrator.run(&mut *conn, &Plan::apply_all()).await?;
        drop(conn);

        let ctx = ServiceContext::new(pool.clone(), pool.clone()).await?;
        let auth = AuthState {
            read_pool: pool.clone(),
            write_pool: pool.clone(),
        };

        Ok(Self {
            dir,
            pool,
            state: CustomerState { ctx, auth },
        })
    }

    async fn register(&self, email: &str) -> Result<String, RegisterError> {
        register_customer(
            &self.state.ctx.executor,
            &self.state.auth.write_pool,
            email,
            "Jane Doe",
            "long-enough-pass",
        )
        .await
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn registration_creates_the_aggregate_and_login_works() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    let id = db.register("Jane@Example.COM").await?;
    let view = load_customer(&db.state.ctx.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("registered customer should replay"))?;
    assert_eq!(view.email, "jane@example.com", "email is normalized");
    assert_eq!(view.full_name, "Jane Doe");

    let (login_id, token) =
        login_customer(&db.state.auth, "jane@example.com", "long-enough-pass").await?;
    assert_eq!(login_id, id);

    // The session token resolves back to the customer through the cookie jar.
    let jar = CookieJar::new().add(Cookie::new(SESSION_COOKIE, token));
    let current = current_customer(&db.state, &jar)
        .await?
        .ok_or_else(|| anyhow::anyhow!("session should resolve to the customer"))?;
    assert_eq!(current.id, id);

    // No cookie, no customer.
    assert!(
        current_customer(&db.state, &CookieJar::new())
            .await?
            .is_none()
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn duplicate_emails_and_bad_credentials_are_refused() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    db.register("jane@example.com").await?;
    assert!(matches!(
        db.register("JANE@example.com").await,
        Err(RegisterError::EmailTaken)
    ));

    assert!(matches!(
        login_customer(&db.state.auth, "jane@example.com", "wrong-password").await,
        Err(LoginError::BadCredentials)
    ));
    assert!(matches!(
        login_customer(&db.state.auth, "nobody@example.com", "long-enough-pass").await,
        Err(LoginError::BadCredentials)
    ));

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn weak_registrations_are_refused() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    let short = register_customer(
        &db.state.ctx.executor,
        &db.state.auth.write_pool,
        "jane@example.com",
        "Jane Doe",
        "short",
    )
    .await;
    assert!(matches!(short, Err(RegisterError::Invalid(_))));

    let bad_email = register_customer(
        &db.state.ctx.executor,
        &db.state.auth.write_pool,
        "not-an-email",
        "Jane Doe",
        "long-enough-pass",
    )
    .await;
    assert!(matches!(bad_email, Err(RegisterError::Invalid(_))));

    db.close().await;
    Ok(())
}
