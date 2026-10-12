//! The topcoat router. This module maps to `/`; child modules add paths.

pub mod admin;
pub mod auth;
pub mod invitations;
pub mod t;
pub mod tenants;

use std::sync::Arc;
use std::time::Duration;

use timada_tenancy::tenants_of_user;
use topcoat::{
    Result,
    asset::{AssetBundle, RouterBuilderAssetExt},
    context::{Cx, app_context},
    cookie::{Key, RouterBuilderCookieExt},
    mail::RouterBuilderMailExt,
    router::{
        Router, RouterBuilderDiscoverExt, TrustedProxies, href, module_router, page, query_params,
    },
    runtime::RouterBuilderRuntimeExt,
    session::{RouterBuilderSessionExt, SessionConfig},
    view::{View, view},
};
use webauthn_rs::prelude::Webauthn;

use crate::auth::mail::Mailer;
use crate::auth::oidc::Providers;
use crate::authz::Authorizer;
use crate::cluster::Executor;
use crate::config::Config;
use crate::db::{ReadPool, WritePool};
use crate::store_api::StoreConnections;
use crate::tenants::Tenants;
use crate::ui::{document, notice};

/// Everything the pages reach through the app context.
pub struct Services {
    pub executor: Executor,
    pub read_pool: ReadPool,
    pub write_pool: WritePool,
    pub providers: Providers,
    pub webauthn: Webauthn,
    pub mailer: Mailer,
    pub tenants: Arc<Tenants>,
    pub authorizer: Arc<Authorizer>,
    /// The live store connections, so revoking a key ends them.
    pub store: Arc<StoreConnections>,
    pub config: Config,
}

pub fn router(services: Services) -> Router {
    let Services {
        executor,
        read_pool,
        write_pool,
        providers,
        webauthn,
        mailer,
        tenants,
        authorizer,
        store,
        config,
    } = services;
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
        .app_context(tenants)
        .app_context(authorizer)
        .app_context(store)
        // The transport behind `topcoat::mail::send`, and who the codes come from.
        .mail(mailer.transport)
        .app_context(mailer.sender)
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
    let mine = tenants_of_user(app_context::<ReadPool>(cx), &user.id).await?;
    Ok(view! {
        document(
            title: "Timada dashboard",
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Timada dashboard"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <span>"Signed in as " <strong>(&user.label)</strong></span>
                    if user.is_admin {
                        <a class="underline" href=(href!(admin::tenants::tenants))>"Tenants"</a>
                        <a class="underline" href=(href!(admin::users::users))>"Users"</a>
                    }
                    <a class="underline" href=(href!(auth::account::account))>"Account"</a>
                    <form method="post" action=(href!(auth::logout::logout))>
                        <button class="underline">"Sign out"</button>
                    </form>
                </nav>
            </header>
            notice(message: query.message.as_deref())

            <section class="rounded border border-neutral-200 bg-white p-4">
                <div class="mb-2 flex items-center justify-between">
                    <h2 class="text-lg font-medium">"Your tenants"</h2>
                    <a class="rounded bg-neutral-900 px-3 py-1 text-sm font-medium text-white hover:bg-neutral-700" href=(href!(tenants::new::new))>"Create a tenant"</a>
                </div>
                if mine.is_empty() {
                    <p class="text-sm text-neutral-500">"You do not belong to any tenant yet. Create one, or ask an owner to invite you."</p>
                } else {
                    <ul class="divide-y divide-neutral-100 text-sm">
                        for row in &mine {
                            <li class="flex items-center justify-between py-2">
                                <a class="underline" href=(href!(t::slug::index, t::slug::Slug(row.slug.clone())))>(&row.name)</a>
                                <span class="text-neutral-500">
                                    (&row.role)
                                    if row.suspended { " · suspended" }
                                </span>
                            </li>
                        }
                    </ul>
                }
            </section>
        )
    })
}
