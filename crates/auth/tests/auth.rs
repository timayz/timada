//! Auth against a real SQLite database: hashing, sessions, admin users and
//! the middleware gate. Argon2 and the cookie flow are exactly what runs in
//! production, so mocking any of it would prove nothing.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Router, middleware};
use sqlx::SqlitePool;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_auth::{
    AuthState, SESSION_COOKIE, SessionKind, create_admin_user, create_session, delete_session,
    hash_password, load_session, migrations, require_admin, session_token, verify_admin,
    verify_password,
};
use timada_core::new_id;
use tower::ServiceExt as _;

/// A temp-file database plus the auth state, torn down on drop.
struct TestDb {
    dir: std::path::PathBuf,
    pool: SqlitePool,
    state: AuthState,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-auth-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());

        // One pool for both roles: these tests are single-threaded CLI-shaped
        // work, and a read-only pool could not run the migrations.
        let pool = timada_core::db::create_pool(&url, 1).await?;

        let mut migrator = Migrator::<sqlx::Sqlite>::default();
        migrator.add_migrations(migrations())?;
        let mut conn = pool.acquire().await?;
        migrator.run(&mut *conn, &Plan::apply_all()).await?;
        drop(conn);

        Ok(Self {
            state: AuthState {
                read_pool: pool.clone(),
                write_pool: pool.clone(),
            },
            dir,
            pool,
        })
    }

    async fn close(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn passwords_verify_only_with_the_right_plaintext() -> anyhow::Result<()> {
    let hash = hash_password("s3cret-enough")?;
    assert!(hash.starts_with("$argon2"), "expected a PHC string: {hash}");
    assert!(verify_password("s3cret-enough", &hash)?);
    assert!(!verify_password("not-it", &hash)?);
    assert!(verify_password("anything", "not-a-phc-string").is_err());
    Ok(())
}

#[tokio::test]
async fn sessions_expire_revoke_and_stay_kind_scoped() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    let token = create_session(&db.state.write_pool, "admin-1", SessionKind::Admin, 60_000).await?;
    let session = load_session(&db.state.read_pool, &token, SessionKind::Admin)
        .await?
        .ok_or_else(|| anyhow::anyhow!("fresh session should load"))?;
    assert_eq!(session.subject_id, "admin-1");

    // The kind column is a hard wall, not a hint.
    assert!(
        load_session(&db.state.read_pool, &token, SessionKind::Customer)
            .await?
            .is_none()
    );

    // An already-expired session never loads.
    let stale = create_session(&db.state.write_pool, "admin-1", SessionKind::Admin, -1).await?;
    assert!(
        load_session(&db.state.read_pool, &stale, SessionKind::Admin)
            .await?
            .is_none()
    );

    // Logout revokes server-side, and twice is fine.
    delete_session(&db.state.write_pool, &token).await?;
    delete_session(&db.state.write_pool, &token).await?;
    assert!(
        load_session(&db.state.read_pool, &token, SessionKind::Admin)
            .await?
            .is_none()
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn admin_users_upsert_and_verify() -> anyhow::Result<()> {
    let db = TestDb::new().await?;

    let id = create_admin_user(&db.state.write_pool, "Boss@Example.COM", "first-pass").await?;
    assert_eq!(
        verify_admin(&db.state.read_pool, "boss@example.com", "first-pass").await?,
        Some(id.clone()),
        "email comparison must be case-insensitive"
    );
    assert_eq!(
        verify_admin(&db.state.read_pool, "boss@example.com", "wrong").await?,
        None
    );
    assert_eq!(
        verify_admin(&db.state.read_pool, "nobody@example.com", "first-pass").await?,
        None
    );

    // Re-creating the same email resets the password, keeps the identity.
    let same_id =
        create_admin_user(&db.state.write_pool, "boss@example.com", "second-pass").await?;
    assert_eq!(same_id, id);
    assert_eq!(
        verify_admin(&db.state.read_pool, "boss@example.com", "second-pass").await?,
        Some(id)
    );
    assert_eq!(
        verify_admin(&db.state.read_pool, "boss@example.com", "first-pass").await?,
        None
    );

    db.close().await;
    Ok(())
}

/// A one-route app behind `require_admin`, the same wiring as the demo store.
fn gated_app(state: AuthState) -> Router {
    Router::new()
        .route("/admin", get(|| async { "dashboard".into_response() }))
        .layer(middleware::from_fn_with_state(state, require_admin))
}

#[tokio::test]
async fn middleware_redirects_anonymous_and_passes_admins() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let app = gated_app(db.state.clone());

    // No cookie → straight to the login form.
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/admin").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/admin/login")
    );

    // A live admin session passes through.
    let id = create_admin_user(&db.state.write_pool, "boss@example.com", "pass").await?;
    let token = create_session(&db.state.write_pool, &id, SessionKind::Admin, 60_000).await?;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin")
                .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    // A customer session must not open the admin.
    let customer_token = create_session(
        &db.state.write_pool,
        "cust-1",
        SessionKind::Customer,
        60_000,
    )
    .await?;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/admin")
                .header(header::COOKIE, format!("{SESSION_COOKIE}={customer_token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn session_token_reads_the_cookie_jar() {
    use axum_extra::extract::cookie::{Cookie, CookieJar};

    let jar = CookieJar::new().add(Cookie::new(SESSION_COOKIE, "abc123"));
    assert_eq!(session_token(&jar), Some("abc123".to_owned()));
    assert_eq!(session_token(&CookieJar::new()), None);
}
