//! The admin mounted into an axum host with `mount_axum` (feature `axum`).

#![cfg(feature = "axum")]

use timada_admin::{AdminConfig, AdminServices, Stylesheet, migrations, mount_axum};
use topcoat::asset::{AssetCatalog, AssetConfig};
use tower::ServiceExt;

#[tokio::test]
async fn admin_routes_are_served_next_to_the_hosts() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let host = axum::Router::new().route("/", axum::routing::get(|| async { "storefront" }));
    let app = mount_axum(
        host,
        AdminConfig {
            mount: "admin".into(),
            stylesheet: Stylesheet::Url("/dev.css".into()),
            ..AdminConfig::default()
        },
        AssetConfig::hosted_at("/assets", AssetCatalog::default()),
        AdminServices::new(executor, db),
    );

    let response = app
        .clone()
        .oneshot(http::Request::get("/").body(axum::body::Body::empty())?)
        .await?;
    assert_eq!(response.status(), 200);

    let response = app
        .clone()
        .oneshot(http::Request::get("/admin/login").body(axum::body::Body::empty())?)
        .await?;
    assert_eq!(response.status(), 200);

    let response = app
        .clone()
        .oneshot(http::Request::get("/admin").body(axum::body::Body::empty())?)
        .await?;
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/admin/orders");

    let response = app
        .oneshot(http::Request::get("/admin/orders").body(axum::body::Body::empty())?)
        .await?;
    assert_eq!(response.status(), 303);
    Ok(())
}
