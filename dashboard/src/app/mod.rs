//! The topcoat router. This module maps to `/`; child modules add paths.

pub mod admin;
pub mod aliexpress;
pub mod auth;
pub mod cluster;
pub mod companies;
pub mod invitations;
mod server_errors;
pub mod store;
pub mod stripe;
pub mod t;
pub mod tenants;
pub mod ui;

use std::sync::Arc;
use std::time::Duration;

use timada_tenancy::tenants_of_user;
use topcoat::{
    Result,
    asset::{AssetBundle, RouterBuilderAssetExt},
    context::{Cx, app_context},
    cookie::{Key, RouterBuilderCookieExt},
    icon::icon,
    mail::RouterBuilderMailExt,
    router::{
        BodyLimit, Router, RouterBuilderDirectoryExt, RouterBuilderDiscoverExt, TrustedProxies,
        href, module_router, page, query_params,
    },
    runtime::RouterBuilderRuntimeExt,
    session::{RouterBuilderSessionExt, SessionConfig},
    view::{View, attributes, class, view},
};
use webauthn_rs::prelude::Webauthn;

use crate::auth::oidc::Providers;
use crate::authz::Authorizer;
use crate::cluster::Cluster;
use crate::cluster::Executor;
use crate::components::avatar::{AvatarSize, avatar, avatar_fallback};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::card::CARD;
use crate::config::Config;
use crate::db::{ReadPool, WritePool};
use crate::key_vault::{self, KeyVault};
use crate::mail::Mailer;
use crate::media::{MEDIA_ROUTE, Media, MediaCache, PEER_ROUTE};
use crate::rate_limit::RateLimiter;
use crate::store_api::StoreConnections;
use crate::ui::chrome::initials;
use crate::ui::{Shell, Tone, document, empty_state, icons, notice, status_badge};

/// The key of the links that stop a shop's automated mails, when the
/// operator gave a secret ([`crate::seal::opt_out_key`]).
#[derive(Clone, Copy, Debug)]
pub struct OptOutLinks(pub Option<crate::seal::OptOutKey>);

/// What the scenarios' webhook steps are sealed and signed with, when the
/// operator gave a secret ([`crate::seal::webhook_keys`]).
#[derive(Clone, Debug)]
pub struct WebhookSealing(pub Option<crate::seal::WebhookKeys>);

/// Everything the pages reach through the app context.
pub struct Services {
    pub executor: Executor,
    pub read_pool: ReadPool,
    pub write_pool: WritePool,
    pub providers: Providers,
    pub webauthn: Webauthn,
    pub mailer: Mailer,
    /// The consensus node, for the cluster page.
    pub cluster: Arc<Cluster>,
    pub authorizer: Arc<Authorizer>,
    /// Where the product pictures are kept and served from.
    pub media: Arc<Media>,
    /// Where the keys personal data is sealed under are kept.
    pub keys: Arc<KeyVault>,
    /// What checks the mailed codes of the platform's users and of the shops'
    /// shoppers.
    pub email_codes: crate::seal::EmailCodes,
    pub shopper_codes: crate::seal::ShopperCodes,
    /// The tenants' Stripe accounts and the provider they are reached through.
    pub payments: Arc<crate::payments::Payments>,
    /// The shops' own mail servers.
    pub shop_mail: crate::shop_mail::ShopMail,
    /// The shops' own AliExpress apps.
    pub aliexpress: Arc<crate::aliexpress::Aliexpress>,
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
        cluster,
        authorizer,
        media,
        keys,
        email_codes,
        shopper_codes,
        payments,
        shop_mail,
        aliexpress,
        store,
        config,
    } = services;
    // The bundle sits next to the binary; `topcoat dev` writes it after every
    // build. A bare `cargo run` has none, and rendering an asset without it
    // panics with a less helpful message than this one.
    let assets = AssetBundle::load().expect(
        "asset bundle missing: run `topcoat dev` or `topcoat asset bundle -p timada-dashboard`",
    );

    // A picture is at most 8 MiB, and a multipart body carries a little more
    // than the file it holds.
    const UPLOAD_LIMIT: usize = 10 * 1024 * 1024;
    // A review carries up to three of them.
    const REVIEW_LIMIT: usize = 26 * 1024 * 1024;

    module_router!()
        .discover()
        // Product pictures: public, unauthenticated, cached forever. An id is
        // an unguessable ULID and the file at an address never changes.
        .serve_dir(MEDIA_ROUTE, media.dir().to_path_buf())
        .layer(MediaCache::new())
        .layer(BodyLimit::max(UPLOAD_LIMIT).at("/t"))
        // A picture another node copies here: the file alone.
        .layer(BodyLimit::max(UPLOAD_LIMIT).at(PEER_ROUTE))
        // A key another node copies here.
        .layer(BodyLimit::max(key_vault::MAX_SEALED_LEN).at(key_vault::PEER_ROUTE))
        .layer(BodyLimit::max(REVIEW_LIMIT).at("/store/reviews"))
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
        .app_context(cluster)
        .app_context(authorizer)
        .app_context(media)
        .app_context(keys)
        .app_context(email_codes)
        .app_context(shopper_codes)
        .app_context(payments)
        .app_context(aliexpress)
        .app_context(store)
        // How often each storefront and shopper has had a sign-in code mailed
        // from this node.
        .app_context(Arc::new(RateLimiter::new()))
        // The transport behind `topcoat::mail::send`, and who the codes come from.
        .mail(mailer.transport)
        .app_context(mailer.sender)
        .app_context(shop_mail)
        // Signs and checks the links that stop a shop's automated mails;
        // none without `secrets.key`.
        .app_context(OptOutLinks(crate::seal::opt_out_key(&config)))
        // Seals the addresses of the scenarios' webhooks and derives their
        // signing secrets; none without `secrets.key`.
        .app_context(WebhookSealing(crate::seal::webhook_keys(&config)))
        .app_context(config)
        .assets(assets)
        .runtime()
        // Outside every other layer, so it sees what any of them failed with.
        .layer(server_errors::LogServerErrors)
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
    let tenants_line = match mine.len() {
        0 => "You do not belong to any tenant yet.".to_owned(),
        1 => "You belong to one tenant.".to_owned(),
        n => format!("You belong to {n} tenants."),
    };
    Ok(view! {
        document(
            title: "Timada dashboard",
            shell: Shell::page("dashboard"),
            notice(message: query.message.as_deref())

            <section class=(class!(CARD, "mb-6 flex-row flex-wrap items-center gap-4 border-primary/20 bg-primary/5 px-6 py-5"))>
                avatar(
                    size: AvatarSize::Lg,
                    avatar_fallback(attrs: attributes! { class="bg-primary/10 text-base text-primary" }, (initials(&user.label)))
                )
                <div class="min-w-0 flex-1">
                    <h1 class="text-2xl font-bold tracking-tight">"Welcome back, " (&user.label) "!"</h1>
                    <p class="mt-0.5 text-sm text-muted-foreground">(tenants_line)</p>
                </div>
                <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Md)) href=(href!(tenants::new::new))>
                    icon(data: icons::PLUS)
                    "Create a tenant"
                </a>
            </section>

            <h2 class="mb-3 text-base font-semibold">"Your tenants"</h2>
            if mine.is_empty() {
                <section class=(CARD)>
                    empty_state(
                        title: "No tenant yet",
                        hint: Some("Create one, or ask an owner to invite you."),
                        symbol: Some(icons::TENANTS),
                    )
                </section>
            } else {
                <ul class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                    for row in &mine {
                        <li>
                            <a
                                class=(class!(CARD, "h-full gap-3 px-5 py-5 transition-all hover:-translate-y-0.5 hover:border-primary/40 hover:shadow-lg motion-reduce:transition-none"))
                                href=(href!(t::slug::index, t::slug::Slug(row.slug.clone())))
                            >
                                <span class="flex items-center gap-3">
                                    <span class="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary [&>svg]:size-4">
                                        icon(data: icons::TENANTS)
                                    </span>
                                    <span class="min-w-0 truncate font-semibold">(&row.name)</span>
                                </span>
                                <span class="flex flex-wrap items-center gap-2">
                                    badge(variant: BadgeVariant::Secondary, (&row.role))
                                    if row.suspended {
                                        status_badge(tone: Tone::Warning, "suspended")
                                    }
                                </span>
                            </a>
                        </li>
                    }
                </ul>
            }
        )
    })
}
