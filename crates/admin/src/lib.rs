//! A topcoat admin for the timada contexts that end users mount into their
//! own app — a topcoat app through [`mount`], any tower/axum app through
//! [`service`] (or [`mount_axum`] with the `axum` feature).
//!
//! The admin is a self-contained topcoat [`Router`]: its own layout, session,
//! auth layer and app context. Its pages are module-derived under this
//! crate's own root, so a host's `Router::builder().discover()` does not pick
//! them up; the URL segment they live under is renamed at runtime from
//! [`AdminConfig::mount`], so the routes are registered under the real prefix
//! and every generated href, redirect and asset URL is correct without any
//! prefix stripping. The admin walks the link-time inventory itself, keeping
//! only handlers under its own root, so a host's own module-derived handlers
//! and `path_param!` segments never trip it up. (The reverse still holds: a
//! host that uses `module_router!()` panics on the admin's handlers until
//! topcoat's module discovery skips foreign roots.)

mod app;
mod auth;
pub mod components;
mod config;
mod error;
mod migration;
mod ui;

use std::borrow::Cow;

use topcoat::{
    asset::{AssetConfig, RouterBuilderAssetExt},
    cookie::RouterBuilderCookieExt,
    router::{
        ModuleLayer, ModuleLayout, ModulePage, ModuleRoute, ModuleRouterBuilder, PathBuf,
        PathSegment, Router, RouterBuilder, Segment,
        tower::{TowerRoute, TowerService},
    },
    session::{RouterBuilderSessionExt, SessionConfig, cookie::CookieTokenStore},
};

pub use auth::{AdminUser, Role, Section, create_admin, create_operator, team};
pub use config::{AdminConfig, AdminServices, Stylesheet};
pub use error::AdminError;
pub use migration::migrations;

/// Name of the session cookie (served as `__Host-timada_admin`), distinct
/// from whatever the host uses for its own sessions.
pub const SESSION_COOKIE: &str = "timada_admin";

/// Builds the admin router. `assets` is the host's asset bundle (or an
/// `AssetConfig::hosted_at(..)` when assets are served elsewhere).
pub fn router(
    config: AdminConfig,
    assets: impl Into<AssetConfig>,
    services: AdminServices,
) -> Router {
    let mount = Segment::new(
        "timada_admin::app::admin",
        None,
        Some(Cow::Owned(config.mount.clone())),
    );
    let sessions = SessionConfig::builder()
        .token_store(CookieTokenStore::new().name(SESSION_COOKIE))
        .build();

    module_router(ROOT, mount)
        .app_context(config)
        .app_context(services)
        .cookies()
        .sessions(sessions)
        .assets(assets)
        .build()
}

/// Root of the admin's module-derived route tree.
const ROOT: &str = "timada_admin::app";

/// `ModuleRouterBuilder::discover()` restricted to handlers declared under
/// `root`: the stock discovery panics on any module-derived handler or
/// `path_param!` segment elsewhere in the binary, i.e. in the host.
fn module_router(root: &'static str, mount: Segment) -> RouterBuilder {
    let under_root = |module_path: &str| {
        module_path == root
            || module_path
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with("::"))
    };
    let mut builder = ModuleRouterBuilder::new(root).segment(mount);
    for segment in inventory::iter::<Segment>() {
        if under_root(segment.module_path()) {
            builder = builder.segment(segment.clone());
        }
    }
    for &page in inventory::iter::<&'static dyn ModulePage>() {
        if under_root(page.module_path()) {
            builder = builder.page(page);
        }
    }
    for &layout in inventory::iter::<&'static dyn ModuleLayout>() {
        if under_root(layout.module_path()) {
            builder = builder.layout(layout);
        }
    }
    for &route in inventory::iter::<&'static dyn ModuleRoute>() {
        if under_root(route.module_path()) {
            builder = builder.route(route);
        }
    }
    for &layer in inventory::iter::<&'static dyn ModuleLayer>() {
        if under_root(layer.module_path()) {
            builder = builder.layer(layer);
        }
    }
    builder.into()
}

/// The admin as a tower service, for hosts that own the HTTP server.
/// Mount it where the host forwards full request paths (never behind a
/// prefix-stripping nest): `/{mount}`, `/{mount}/{*rest}` and, unless the
/// host serves the asset bundle itself, `/_topcoat/assets/{*rest}`.
pub fn service(
    config: AdminConfig,
    assets: impl Into<AssetConfig>,
    services: AdminServices,
) -> TowerService {
    TowerService::new(router(config, assets, services))
}

/// Mounts the admin into a topcoat host router. The host is expected to
/// serve the shared asset bundle at its default `/_topcoat/assets` route.
pub fn mount(
    builder: RouterBuilder,
    config: AdminConfig,
    assets: impl Into<AssetConfig>,
    services: AdminServices,
) -> RouterBuilder {
    let (root, rest) = mount_paths(&config.mount);
    let admin = service(config, assets, services);
    builder
        .route(TowerRoute::any(root, admin.clone()))
        .route(TowerRoute::any(rest, admin))
}

/// `/{mount}` and `/{mount}/{*rest}`.
fn mount_paths(mount: &str) -> (PathBuf, PathBuf) {
    let mut root = PathBuf::new();
    root += PathSegment::Static(mount);
    let mut rest = root.clone();
    rest += PathSegment::CatchAll("rest");
    (root, rest)
}

/// Mounts the admin into an axum router (feature `axum`), forwarding the
/// admin prefix and the asset route without stripping anything.
#[cfg(feature = "axum")]
pub fn mount_axum(
    app: axum::Router,
    config: AdminConfig,
    assets: impl Into<AssetConfig>,
    services: AdminServices,
) -> axum::Router {
    let prefix = config.prefix();
    let admin = service(config, assets, services);
    app.route_service(&prefix, admin.clone())
        .route_service(&format!("{prefix}/{{*rest}}"), admin.clone())
        .route_service("/_topcoat/assets/{*rest}", admin)
}
