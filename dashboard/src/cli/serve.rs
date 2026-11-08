use std::sync::Arc;

use anyhow::{Context, Result};
use evento::subscription::SubscriptionBuilder;

use crate::app::{self, Services};
use crate::auth;
use crate::cluster::{self, Executor};
use crate::config::Config;
use crate::db;
use crate::store_api::{self, StoreConnections};
use crate::workers::{self, Workers};

pub async fn serve(config: Config, host: Option<String>, port: Option<u16>) -> Result<()> {
    let host = host.unwrap_or_else(|| config.server.host.clone());
    let port = port.unwrap_or(config.server.port);
    let url = config.server.url.clone();

    // The store first: its node may have to copy it from a peer, which
    // recreates the read models, so their pools open afterwards. Write pool
    // before read pool: it creates the file a read-only connection cannot.
    let store = db::open_store(&config.database.path).await?;
    let (cluster, executor) = cluster::open(
        config.cluster.as_ref(),
        config.server.region.as_deref(),
        &config.database.path,
        store.clone(),
    )
    .await?;
    // The read models in the store's database.
    let db = db::Db::from(store.clone());

    let peers = crate::peers::Peers::from_cluster(config.cluster.as_ref())?.map(Arc::new);
    let media = Arc::new(crate::media::Media::new(&config.media, peers.clone()));
    let keys_sealer = crate::seal::keys_sealer(&config);
    if keys_sealer.is_none() {
        tracing::warn!(
            "secrets.key is not set: no shopper can create an account and no contact message can be received"
        );
    }
    let keys = Arc::new(crate::key_vault::KeyVault::new(
        &config.data_keys,
        keys_sealer,
        peers.clone(),
    ));
    // The sign-in, code, contact and sign-up limits, held with the other
    // nodes.
    let limits = Arc::new(crate::rate_limit::RateLimiter::new(peers));
    let gateway = Arc::new(crate::payments::StripeGateway::new(
        config
            .payments
            .as_ref()
            .and_then(|payments| payments.stripe_api_url.clone()),
    ));
    let payments = Arc::new(crate::payments::Payments::new(
        crate::seal::payments_sealer(&config),
        gateway,
        executor.clone(),
    ));
    // The shops' own mail servers, shared by the pages and the mails sent
    // in the background so their connections are too.
    let shop_mail = crate::shop_mail::ShopMail::new(
        crate::seal::mail_sealer(&config),
        config.mail.allow_private_hosts,
    );
    let live = cluster.clone();
    let aliexpress = crate::aliexpress::Aliexpress::new(
        crate::seal::aliexpress_sealer(&config),
        &config.aliexpress,
        config.mail.allow_private_hosts,
        executor.clone(),
        Arc::new(move || {
            let (index, of) = live.live_nodes(crate::aliexpress::LIVE_WITHIN);
            timada_aliexpress::Share { index, of }
        }),
    )?;
    // The order confirmations and the notices of contact messages are
    // mailed from the subscriptions, outside any request: they get their
    // own transport.
    let background_mail = {
        let mailer = crate::mail::build(&config.mail)?;
        let mut app = topcoat::context::AppContext::new();
        app.insert(mailer.transport);
        app.insert(mailer.sender);
        app.insert(shop_mail.clone());
        // Where the shops' logos are loaded from.
        app.insert(media.clone());
        Arc::new(app)
    };
    let connections = StoreConnections::new();
    // This node's store connections of a key revoked or re-scoped, or of a
    // tenant suspended, on any node, end through this subscription. It starts
    // before the store API takes a connection, so none is older than it.
    let mut all_workers = vec![
        workers::start(
            "store connections",
            {
                let connections = connections.clone();
                move || store_api::closing_subscription(connections.clone())
            },
            &executor,
        )
        .await?,
    ];
    let store_api = store_api::serve(
        config.store_api.as_ref(),
        executor.clone(),
        connections.clone(),
    )
    .await?;
    let authorizer = crate::authz::Authorizer::new().await?;

    // Sign-in providers are resolved through discovery now, so a bad issuer
    // or network stops the server with the provider's name instead of
    // failing a sign-in later.
    let providers = auth::oidc::Providers::discover(&config.auth, &config.server.url).await?;
    let webauthn = auth::webauthn::build(&config.auth, &config.server.url)?;
    let mailer = crate::mail::build(&config.mail)?;

    // Every subscription follows the stream from its saved cursor under a
    // supervisor (`workers`): a worker that fails is started again from the
    // event that failed, and the cluster page says so. After this start
    // copied the store from a peer, the read models are empty: each
    // subscription first catches up over the copied events, so the pages
    // never show an empty shop, and the router takes requests afterwards.
    let deps = Arc::new(Deps {
        db: db.clone(),
        store: executor.clone(),
        media: media.clone(),
        keys: keys.clone(),
        payments: payments.clone(),
        aliexpress: aliexpress.clone(),
        vies: Arc::new(crate::vies::ViesClient::new(
            config.invoicing.vies_url.clone(),
        )),
        euro_rates: Arc::new(crate::ecb::EcbClient::new(
            config.invoicing.euro_rates_url.clone(),
        )),
        background_mail: background_mail.clone(),
        base_url: url.clone(),
        node: cluster::node_name(&cluster),
    });
    let catch_up_first = cluster.health().joined_at_start;
    if catch_up_first {
        tracing::info!("the store was copied from a peer: the read models catch up before serving");
    }
    for (name, make) in SUBSCRIPTIONS {
        if catch_up_first {
            workers::catch_up(name, make(&deps), &executor).await?;
        }
        let deps = deps.clone();
        all_workers.push(workers::start(name, move || make(&deps), &executor).await?);
    }
    let workers = Workers::new(all_workers);

    // The operator's alert for a node that stays away: this node nearing the
    // writes it may keep for it, or a node given up on.
    let cluster_watch = cluster.watch(std::time::Duration::from_secs(60));
    // A few days after an order left, its buyer is asked for a review.
    let review_requests = crate::review_requests::ReviewRequests::new(
        db.clone(),
        executor.clone(),
        background_mail.clone(),
    )
    .start(std::time::Duration::from_secs(
        config.reviews.request_check_secs.max(1),
    ));

    // A card form left open past the time its goods were held for is closed
    // with Stripe before it can be confirmed; what an order asked of Stripe
    // and got no answer to is settled, and a request to pay nobody answered
    // is refused, on the same beat.
    let payments_every = std::time::Duration::from_secs(
        config
            .payments
            .as_ref()
            .map_or(60, |payments| payments.cancel_check_secs)
            .max(1),
    );
    let lapsed_checkouts =
        crate::payments::LapsedCheckouts::new(db.clone(), executor.clone(), payments.clone())
            .start(payments_every);
    let pending_payments =
        crate::payments::PendingPayments::new(db.clone(), executor.clone(), payments.clone())
            .start(payments_every);
    let unanswered_checkouts =
        crate::payments::UnansweredCheckouts::new(db.clone(), executor.clone())
            .start(payments_every);
    // Two days before the hold on a shopper's card lapses on an order
    // AliExpress may have, the members who manage orders are reminded to
    // take the payment.
    let hold_reminders = crate::hold_reminders::HoldReminders::new(
        db.clone(),
        executor.clone(),
        crate::hold_reminders::PlatformMail(background_mail.clone()),
        url.clone(),
    )
    .start(payments_every);

    // A campaign a merchant sent goes out a batch at a time.
    let newsletter_sends = crate::newsletter_sends::NewsletterSends::new(
        db.clone(),
        executor.clone(),
        background_mail.clone(),
        keys.clone(),
        config.newsletter.mails_per_check,
    )
    .start(std::time::Duration::from_secs(
        config.newsletter.send_check_secs.max(1),
    ));

    // The runs of the shops' automation scenarios move on a timer.
    let automation_runs = crate::automations::runner::Runner::new(
        db.clone(),
        executor.clone(),
        background_mail.clone(),
        url.clone(),
        keys.ring(),
        crate::seal::opt_out_key(&config),
        crate::seal::webhook_keys(&config),
        std::time::Duration::from_secs(config.automations.webhook_timeout_secs.max(1)),
        config.mail.allow_private_hosts,
        std::time::Duration::from_secs(config.automations.abandoned_after_secs.max(60)),
        config.automations.runs_per_check,
    )?
    .start(std::time::Duration::from_secs(
        config.automations.check_secs.max(1),
    ));

    // The products imported from AliExpress are read again as the day's calls
    // allow, and the accounts and open orders on a timer.
    let aliexpress_sync = crate::aliexpress::sync::AliexpressSync::new(
        db.clone(),
        executor.clone(),
        aliexpress.clone(),
    )
    .start(std::time::Duration::from_secs(
        config.aliexpress.sync_check_secs.max(1),
    ));

    let router = app::router(Services {
        executor,
        db,
        providers,
        webauthn,
        mailer,
        cluster: cluster.clone(),
        authorizer,
        media,
        keys,
        limits,
        email_codes: crate::seal::email_codes(&config),
        shopper_codes: crate::seal::shopper_codes(&config),
        payments,
        shop_mail,
        aliexpress,
        store: connections,
        workers: workers.clone(),
        config,
    });

    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("cannot bind {host}:{port}"))?;
    tracing::info!(addr = %listener.local_addr()?, "server listening");
    tracing::info!("open {url}");

    // Ctrl+C or SIGTERM starts graceful shutdown; in-flight requests get
    // `RouterService::shutdown_timeout` to finish.
    topcoat::serve(listener, router).await?;

    if let Some(store_api) = store_api {
        store_api.shutdown().await;
    }
    cluster_watch.abort();
    review_requests.abort();
    lapsed_checkouts.abort();
    pending_payments.abort();
    unanswered_checkouts.abort();
    hold_reminders.abort();
    newsletter_sends.abort();
    automation_runs.abort();
    aliexpress_sync.abort();
    workers.shutdown().await;
    cluster.shutdown().await;
    drop(store);
    Ok(())
}

/// What the subscriptions are built from. Cloned into each worker's
/// factory, so a worker that failed is built again from the same parts.
pub struct Deps {
    /// The read models in the store's database.
    pub db: db::Db,
    pub store: Executor,
    pub media: Arc<crate::media::Media>,
    pub keys: Arc<crate::key_vault::KeyVault>,
    pub payments: Arc<crate::payments::Payments>,
    pub aliexpress: Arc<crate::aliexpress::Aliexpress>,
    pub vies: Arc<dyn crate::vies::Vies>,
    pub euro_rates: Arc<dyn crate::ecb::EuroRates>,
    /// The transport and sender the subscriptions mail with, outside any
    /// request.
    pub background_mail: Arc<topcoat::context::AppContext>,
    /// The public address, for the links in the mails.
    pub base_url: String,
    /// This node, as the work it claims names it.
    pub node: String,
}

/// Builds one subscription from the parts.
type Make = fn(&Deps) -> SubscriptionBuilder<Executor>;

/// Every subscription this node runs on the store, each following every
/// tenant's streams: the platform's read models (the users, the tenant
/// catalog, the members' history), the tenants' read models fed into the
/// read-model database (the catalogs, the orders and carts, the customers,
/// the invoices, the reviews, the support messages), this node's copies of
/// the product pictures, review photos and personal-data keys, the answers
/// the dashboard writes back (payments, invoices), the slugs and codes the
/// catalog, the discounts and the pages give back once their holder let go
/// of them, the order confirmations it mails and the notices of contact
/// messages it mails the members. Every bounded context registers its
/// subscription here, under the name the cluster page shows.
const SUBSCRIPTIONS: &[(&str, Make)] = &[
    ("identity", |d| {
        timada_identity::read_model_subscription(d.db.clone())
    }),
    // Not a read model, but the same lifecycle: it frees the index entries
    // of the sign-in identities a user let go of, should the command not
    // have.
    ("identity indexes", |_| {
        timada_identity::index_release_subscription()
    }),
    ("tenancy", |d| {
        timada_tenancy::read_model_subscription(d.db.clone())
    }),
    ("members history", |d| {
        timada_tenancy::member_audit_subscription(d.db.clone())
    }),
    ("AliExpress access", |d| {
        timada_tenancy::aliexpress_accounts_subscription(d.db.clone())
    }),
    // Not a read model, but the same lifecycle: it copies how each tenant's
    // shoppers pay into the tenant's streams, for the storefronts' payment
    // form.
    ("payment setup", |d| {
        crate::payments::setup_subscription(d.payments.clone(), d.store.clone())
    }),
    // Nor this: it grants the seat an accepted invitation promised, should
    // the command not have.
    ("invitation grants", |d| {
        timada_tenancy::invitation_grants_subscription(d.store.clone())
    }),
    // Nor this: it fetches the shops' mail logos this node lacks.
    ("mail logos", |d| {
        crate::media::logo_subscription(d.media.clone())
    }),
    ("catalog", |d| {
        timada_catalog::read_model_subscription(d.db.clone())
    }),
    ("orders", |d| {
        timada_orders::read_model_subscription(d.db.clone())
    }),
    ("shoppers", |d| {
        timada_shoppers::read_model_subscription(d.db.clone(), d.keys.ring())
    }),
    ("discounts", |d| {
        timada_discounts::read_model_subscription(d.db.clone())
    }),
    ("reviews", |d| {
        timada_reviews::read_model_subscription(d.db.clone())
    }),
    ("support", |d| {
        timada_support::read_model_subscription(d.db.clone(), d.keys.ring())
    }),
    ("newsletter", |d| {
        timada_newsletter::read_model_subscription(d.db.clone(), d.keys.ring())
    }),
    ("automations", |d| {
        timada_automations::read_model_subscription(d.db.clone())
    }),
    ("pages", |d| {
        timada_pages::read_model_subscription(d.db.clone())
    }),
    ("invoicing", |d| {
        timada_invoicing::read_model_subscription(d.db.clone())
    }),
    ("AliExpress", |d| {
        timada_aliexpress::read_model_subscription(d.db.clone())
    }),
    ("AliExpress orders", |d| {
        crate::aliexpress::forward::subscription(
            d.aliexpress.clone(),
            d.db.clone(),
            d.payments.clone(),
            d.store.clone(),
        )
    }),
    ("AliExpress products", |d| {
        crate::aliexpress::subscription::subscription(
            d.aliexpress.clone(),
            d.media.clone(),
            d.store.clone(),
        )
    }),
    ("AliExpress reads", |d| {
        crate::aliexpress::subscription::reads_subscription(
            d.aliexpress.clone(),
            d.db.clone(),
            d.store.clone(),
        )
    }),
    ("AliExpress pushes", |d| {
        crate::aliexpress::webhook::pushes_subscription(
            d.aliexpress.clone(),
            d.db.clone(),
            d.store.clone(),
        )
    }),
    ("pictures", |d| crate::media::subscription(d.media.clone())),
    ("shopper keys", |d| {
        crate::shopper_keys::subscription(d.keys.clone())
    }),
    // Nor this: it erases the newsletter subscription a deleted account
    // names.
    ("shopper erasure", |d| {
        crate::shopper_erasure::subscription(d.store.clone(), d.keys.ring())
    }),
    ("message keys", |d| {
        crate::message_keys::subscription(d.keys.clone())
    }),
    ("newsletter keys", |d| {
        crate::newsletter_keys::subscription(d.keys.clone())
    }),
    ("payments", |d| {
        crate::payments::subscription(crate::payments::Checkouts::new(
            d.payments.clone(),
            d.euro_rates.clone(),
            Arc::new(crate::aliexpress::payment_reads::AliexpressGoods::new(
                d.aliexpress.clone(),
                d.db.clone(),
                d.store.clone(),
            )),
            d.store.clone(),
            d.node.clone(),
        ))
    }),
    ("order payments", |d| {
        crate::payments::order_payments_subscription(d.payments.clone(), d.store.clone())
    }),
    ("discount usage", |d| {
        timada_orders::discount_usage_subscription(d.store.clone())
    }),
    ("stock usage", |d| {
        timada_orders::stock_usage_subscription(d.store.clone())
    }),
    ("catalog releases", |d| {
        timada_catalog::release_subscription(d.store.clone())
    }),
    ("discount releases", |d| {
        timada_discounts::release_subscription(d.store.clone())
    }),
    ("page releases", |d| {
        timada_pages::release_subscription(d.store.clone())
    }),
    ("newsletter opt-ins", |d| {
        crate::newsletter::subscription(
            d.store.clone(),
            d.background_mail.clone(),
            d.keys.clone(),
            d.store.clone(),
        )
    }),
    ("automation triggers", |d| {
        crate::automations::triggers::subscription(d.db.clone(), d.store.clone())
    }),
    ("order mails", |d| {
        crate::order_mails::subscription(
            d.store.clone(),
            d.background_mail.clone(),
            d.payments.clone(),
            d.store.clone(),
        )
    }),
    ("message notices", |d| {
        crate::message_notifications::subscription(
            d.store.clone(),
            d.db.clone(),
            d.background_mail.clone(),
            d.base_url.clone(),
            d.keys.ring(),
            d.store.clone(),
        )
    }),
    ("invoices", |d| {
        crate::invoicing::subscription(
            d.store.clone(),
            d.vies.clone(),
            d.euro_rates.clone(),
            d.store.clone(),
        )
    }),
];
