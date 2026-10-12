//! The topcoat router. This module maps to `/`; child modules add paths.

pub mod auth;

use std::time::Duration;

use topcoat::{
    Result,
    asset::{AssetBundle, RouterBuilderAssetExt},
    context::Cx,
    cookie::{Key, RouterBuilderCookieExt},
    router::{
        Router, RouterBuilderDiscoverExt, TrustedProxies, href, module_router, page, query_params,
    },
    runtime::RouterBuilderRuntimeExt,
    session::{RouterBuilderSessionExt, SessionConfig},
    view::{View, view},
};
use webauthn_rs::prelude::Webauthn;

use crate::auth::oidc::Providers;
use crate::cluster::Executor;
use crate::config::Config;
use crate::db::{ReadPool, WritePool};
use crate::ui::{document, notice};

pub fn router(
    executor: Executor,
    read_pool: ReadPool,
    write_pool: WritePool,
    providers: Providers,
    webauthn: Webauthn,
    config: Config,
) -> Router {
    // The bundle sits next to the binary; `topcoat dev` writes it after every
    // build. A bare `cargo run` has none, and rendering an asset without it
    // panics with a less helpful message than this one.
    let assets = AssetBundle::load().expect(
        "asset bundle missing: run `topcoat dev` or `topcoat asset bundle -p timada-dashboard`",
    );

    module_router!()
        .discover()
        // Absolute URLs (the OIDC redirect URI) follow the public address.
        .base_url(config.server.url.as_str())
        // Validated by `Config::load`; `networks` panics on a bad entry.
        .trusted_proxies(TrustedProxies::new().networks(config.server.trusted_proxies.iter()))
        .cookies()
        .sessions(
            SessionConfig::builder()
                .lifetime(Duration::from_secs(config.auth.session_lifetime_secs))
                .build(),
        )
        // Encrypts the in-flight sign-in cookies (`private_cookies`).
        .app_context(Key::derive_from(config.auth.secret_key.as_bytes()))
        .app_context(executor)
        .app_context(read_pool)
        .app_context(write_pool)
        .app_context(providers)
        .app_context(webauthn)
        .app_context(config)
        .assets(assets)
        .runtime()
        .build()
}

#[topcoat::router::query_params(error = bad_request)]
struct HomeQuery {
    /// A confirmation from a redirect, e.g. after switching accounts.
    message: Option<String>,
}

#[page]
pub async fn home(cx: &Cx) -> Result<impl View> {
    let user = crate::auth::require_user(cx).await?;
    let query = query_params::<HomeQuery>(cx)?;
    Ok(view! {
        document(
            title: "Timada dashboard",
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Timada dashboard"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <span>"Signed in as " <strong>(&user.label)</strong></span>
                    <a class="underline" href=(href!(auth::account::account))>"Account"</a>
                    <form method="post" action=(href!(auth::logout::logout))>
                        <button class="underline">"Sign out"</button>
                    </form>
                </nav>
            </header>
            notice(message: query.message.as_deref())
            <p class="text-neutral-600">"Nothing to see yet."</p>
        )
    })
}
