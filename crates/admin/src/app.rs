//! Root of the module-derived route tree (`ModuleRouterBuilder::new("timada_admin::app")`).
//! The root maps to `/` and serves nothing; everything lives under [`admin`],
//! whose URL segment is renamed at runtime from `AdminConfig::mount`.

pub mod admin;
