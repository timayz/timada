//! Drop-in admin UI for timada e-commerce stores.
//!
//! Mount the admin into any axum app:
//!
//! ```rust,ignore
//! let admin = timada_admin::AdminContext::new(evento, read_pool, providers);
//! let app = axum::Router::new()
//!     .nest("/admin", timada_admin::router(admin));
//! ```
//!
//! The admin ships **no authentication**: wrap the returned router with your
//! own middleware (`timada_admin::router(ctx).layer(my_auth_layer)`).
//!
//! All assets (CSS, TwinSpark JS) are embedded in the crate and served from
//! `{base_path}/assets/...` — no asset pipeline is required in the host app.

mod render;
mod routes;

use std::sync::Arc;

pub use render::AdminError;

/// Everything the admin needs from the host application.
#[derive(Clone)]
pub struct AdminContext {
    /// Event-store executor used to dispatch commands.
    pub evento: evento::sql::RwSqlite,
    /// Read-only pool for querying read models.
    pub read_db: sqlx::SqlitePool,
    /// Provider implementations registered by the host.
    pub providers: Arc<timada_provider::ProviderRegistry>,
    /// Absolute mount prefix, used for links and asset URLs in templates.
    /// Defaults to `"/admin"`.
    pub base_path: String,
}

impl AdminContext {
    pub fn new(
        evento: evento::sql::RwSqlite,
        read_db: sqlx::SqlitePool,
        providers: Arc<timada_provider::ProviderRegistry>,
    ) -> Self {
        Self {
            evento,
            read_db,
            providers,
            base_path: "/admin".to_owned(),
        }
    }

    /// Override the mount prefix when the admin is nested somewhere other
    /// than `/admin`. Nested routers cannot see their own mount point, so the
    /// prefix must be provided for templates to emit correct URLs.
    #[must_use]
    pub fn with_base_path(mut self, base_path: impl Into<String>) -> Self {
        self.base_path = base_path.into();
        self
    }
}

/// Build the admin router. State is applied internally, so the result nests
/// directly into any `Router`.
///
/// The router performs no authentication — wrap it with your own layer.
pub fn router(ctx: AdminContext) -> axum::Router {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/", get(routes::dashboard))
        .route("/assets/admin.css", get(routes::admin_css))
        .route("/assets/twinspark.min.js", get(routes::twinspark_js))
        .route(
            "/providers",
            get(routes::providers::index).post(routes::providers::connect),
        )
        .route("/providers/{id}", get(routes::providers::show))
        .route(
            "/providers/{id}/credentials",
            post(routes::providers::save_credentials),
        )
        .route("/providers/{id}/enable", post(routes::providers::enable))
        .route("/providers/{id}/disable", post(routes::providers::disable))
        .with_state(ctx)
}
