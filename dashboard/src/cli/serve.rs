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
        &config.server.also_serves,
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
        Arc::new(move |region| {
            let (index, of) = cluster::live_place(&live, crate::aliexpress::LIVE_WITHIN, region);
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
        // The keys the mailed codes and the invitation links derive from.
        app.insert(crate::seal::email_codes(&config));
        app.insert(crate::seal::shopper_codes(&config));
        app.insert(crate::seal::invitation_tokens(&config));
        Arc::new(app)
    };
    let connections = StoreConnections::new();
    // This node's store connections of a key revoked or re-scoped, or of a
    // tenant suspended, on any node, end through this subscription. It starts
    // before the store API takes a connection, so none is older than it. It
    // follows from the head and keeps no cursor, so a restart after a failure
    // misses what was revoked in between: those connections last until the
    // key is next checked, as they did when the worker simply died.
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
    // The mails the sagas owe go out from here, off the subscriptions'
    // cursors, a lane per shop: one shop's mail server never holds up the
    // others' codes, invitations and confirmations.
    let euro_rates: Arc<dyn crate::ecb::EuroRates> = Arc::new(crate::ecb::EcbClient::new(
        config.invoicing.euro_rates_url.clone(),
    ));
    let mailroom = crate::mailroom::Mailroom::new(
        db.clone(),
        executor.clone(),
        background_mail.clone(),
        keys.clone(),
        payments.clone(),
        url.clone(),
        cluster::node_name(&cluster),
    );
    // The requests to pay are answered off the payments subscription's
    // cursor too, a lane per cart, several carts at once (#467).
    let checkouts = crate::payments::Checkouts::new(
        payments.clone(),
        euro_rates.clone(),
        Arc::new(crate::aliexpress::payment_reads::AliexpressGoods::new(
            aliexpress.clone(),
            db.clone(),
            executor.clone(),
        )),
        executor.clone(),
        db.clone(),
        cluster::node_name(&cluster),
    );
    let deps = Arc::new(Deps {
        db: db.clone(),
        store: executor.clone(),
        mailroom: mailroom.clone(),
        checkouts: checkouts.clone(),
        media: media.clone(),
        keys: keys.clone(),
        payments: payments.clone(),
        aliexpress: aliexpress.clone(),
        vies: Arc::new(crate::vies::ViesClient::new(
            config.invoicing.vies_url.clone(),
        )),
        euro_rates,
    });
    let planned = planned(&config.server);
    let copied = cluster.health().joined_at_start;
    let rebuilt = cluster::catch_up_mark(&config.database.path).exists();
    if copied || rebuilt {
        if copied {
            tracing::info!(
                "the store was copied from a peer: the read models catch up before serving"
            );
        } else {
            tracing::info!("a read model was rebuilt: the read models catch up before serving");
        }
        // The read models alone: a saga resumes from its cursor as it would
        // after any restart, and nothing it does is waited for. All of them
        // at once: each writes tables of its own, and the store's history is
        // long. A pass that cannot get through it is not the node's end: its
        // worker starts anyway, supervised, at the cursor the pass left, and
        // the cluster page says so.
        let mut catching_up = tokio::task::JoinSet::new();
        for (name, kind, make) in &planned {
            if *kind != Kind::ReadModel {
                continue;
            }
            let name = name.clone();
            let builder = make(&deps);
            let executor = executor.clone();
            catching_up.spawn(async move {
                if let Err(err) = workers::catch_up(&name, builder, &executor).await {
                    tracing::error!(error = %format!("{err:#}"), "the {name} subscription did not catch up before serving");
                }
            });
        }
        while let Some(joined) = catching_up.join_next().await {
            if let Err(err) = joined {
                tracing::error!(%err, "a read model's catch-up ended early");
            }
        }
        tracing::info!("the read models caught up");
        if rebuilt && let Err(err) = cluster::clear_catch_up_mark(&config.database.path) {
            tracing::warn!(%err, "cannot remove the catch-up mark; the next start catches up again");
        }
    }
    for (name, _kind, make) in planned {
        let deps = deps.clone();
        all_workers.push(workers::start(name, move || make(&deps), &executor).await?);
    }
    let workers = Workers::new(all_workers);
    let mailroom_sends = mailroom.start();
    let checkout_answers = checkouts.clone().start();
    // What the balancer asks before sending traffic here, and what an
    // operator scrapes.
    let readiness = Arc::new(crate::readiness::Readiness::new(
        db.clone(),
        cluster.clone(),
        workers.clone(),
        connections.clone(),
        config.monitoring.ready_lag(),
    ));
    let metrics = match config.monitoring.metrics_listen {
        Some(listen) => Some(crate::metrics::serve(listen, readiness.clone()).await?),
        None => None,
    };

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
        std::time::Duration::from_secs(config.aliexpress.sync_check_secs.max(1)),
    )
    .start();

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
        invitation_tokens: crate::seal::invitation_tokens(&config),
        payments,
        shop_mail,
        aliexpress,
        store: connections,
        workers: workers.clone(),
        checkouts,
        readiness,
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
    if let Some(metrics) = metrics {
        metrics.shutdown().await;
    }
    cluster_watch.abort();
    mailroom_sends.abort();
    checkout_answers.abort();
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
    /// Where the mails the sagas owe are handed to be sent.
    pub mailroom: Arc<crate::mailroom::Mailroom>,
    /// Where the requests to pay are handed to be answered.
    pub checkouts: Arc<crate::payments::Checkouts>,
    pub media: Arc<crate::media::Media>,
    pub keys: Arc<crate::key_vault::KeyVault>,
    pub payments: Arc<crate::payments::Payments>,
    pub aliexpress: Arc<crate::aliexpress::Aliexpress>,
    pub vies: Arc<dyn crate::vies::Vies>,
    pub euro_rates: Arc<dyn crate::ecb::EuroRates>,
}

/// Builds one subscription from the parts.
type Make = fn(&Deps) -> SubscriptionBuilder<Executor>;

/// A worker built from a subscription: the one the table lists, or its copy
/// for a region this node also serves, filtered on that region's key.
type Planned = (
    String,
    Kind,
    Box<dyn Fn(&Deps) -> SubscriptionBuilder<Executor> + Send + Sync>,
);

/// What a subscription follows, and what it is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Every region's events, into the read-model tables. The builder says
    /// `.any_routing_key()`. After a start that copied the store from a
    /// peer, these catch up before the router takes requests.
    ReadModel,
    /// Every region's events, for an effect each node has for itself: its
    /// copies of pictures, logos and keys. The builder says
    /// `.any_routing_key()`.
    PerNode,
    /// The node's own region (the executor's default routing key, which the
    /// builder leaves alone), and each region of `server.also_serves` as a
    /// worker of its own: a saga, whose work a region's nodes do for it.
    Sharded,
}

/// The workers to start for `server`: every subscription once, and the
/// sharded ones again for each region this node also serves, named after it
/// (`payments for us-east-3`). Without a region, one worker each, following
/// the events written without a key, as before.
fn planned(server: &crate::config::ServerConfig) -> Vec<Planned> {
    debug_assert!(
        server.also_serves.is_empty() || server.region.is_some(),
        "validated by Config::load"
    );
    let mut planned: Vec<Planned> = Vec::new();
    for (name, kind, make) in SUBSCRIPTIONS {
        planned.push((name.to_string(), *kind, Box::new(make)));
        if *kind == Kind::Sharded {
            for region in &server.also_serves {
                let region = region.clone();
                planned.push((
                    format!("{name} for {region}"),
                    *kind,
                    Box::new(move |deps: &Deps| make(deps).routing_key(region.clone())),
                ));
            }
        }
    }
    planned
}

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
/// subscription here, under the name the cluster page shows, saying whether
/// it follows every region or the node's own ([`Kind`]).
const SUBSCRIPTIONS: &[(&str, Kind, Make)] = &[
    ("identity", Kind::ReadModel, |d| {
        timada_identity::read_model_subscription(d.db.clone())
    }),
    // Not a read model, but the same lifecycle: it frees the index entries
    // of the sign-in identities a user let go of, should the command not
    // have.
    ("identity indexes", Kind::Sharded, |_| {
        timada_identity::index_release_subscription()
    }),
    ("tenancy", Kind::ReadModel, |d| {
        timada_tenancy::read_model_subscription(d.db.clone())
    }),
    ("members history", Kind::ReadModel, |d| {
        timada_tenancy::member_audit_subscription(d.db.clone())
    }),
    ("AliExpress access", Kind::ReadModel, |d| {
        timada_tenancy::aliexpress_accounts_subscription(d.db.clone())
    }),
    // Not a read model, but the same lifecycle: it copies how each tenant's
    // shoppers pay into the tenant's streams, for the storefronts' payment
    // form.
    ("payment setup", Kind::Sharded, |d| {
        crate::payments::setup_subscription(d.payments.clone(), d.store.clone())
    }),
    // Nor this: it grants the seat an accepted invitation promised, should
    // the command not have.
    ("invitation grants", Kind::Sharded, |d| {
        timada_tenancy::invitation_grants_subscription(d.store.clone())
    }),
    // Nor this: it fetches the shops' mail logos this node lacks.
    ("mail logos", Kind::PerNode, |d| {
        crate::media::logo_subscription(d.media.clone())
    }),
    ("catalog", Kind::ReadModel, |d| {
        timada_catalog::read_model_subscription(d.db.clone())
    }),
    ("orders", Kind::ReadModel, |d| {
        timada_orders::read_model_subscription(d.db.clone())
    }),
    ("shoppers", Kind::ReadModel, |d| {
        timada_shoppers::read_model_subscription(d.db.clone(), d.keys.ring())
    }),
    ("discounts", Kind::ReadModel, |d| {
        timada_discounts::read_model_subscription(d.db.clone())
    }),
    ("reviews", Kind::ReadModel, |d| {
        timada_reviews::read_model_subscription(d.db.clone())
    }),
    ("support", Kind::ReadModel, |d| {
        timada_support::read_model_subscription(d.db.clone(), d.keys.ring())
    }),
    ("newsletter", Kind::ReadModel, |d| {
        timada_newsletter::read_model_subscription(d.db.clone(), d.keys.ring())
    }),
    ("automations", Kind::ReadModel, |d| {
        timada_automations::read_model_subscription(d.db.clone())
    }),
    ("pages", Kind::ReadModel, |d| {
        timada_pages::read_model_subscription(d.db.clone())
    }),
    ("invoicing", Kind::ReadModel, |d| {
        timada_invoicing::read_model_subscription(d.db.clone())
    }),
    ("AliExpress", Kind::ReadModel, |d| {
        timada_aliexpress::read_model_subscription(d.db.clone())
    }),
    ("AliExpress orders", Kind::Sharded, |d| {
        crate::aliexpress::forward::subscription(
            d.aliexpress.clone(),
            d.db.clone(),
            d.payments.clone(),
            d.store.clone(),
        )
    }),
    ("AliExpress products", Kind::Sharded, |d| {
        crate::aliexpress::subscription::subscription(
            d.aliexpress.clone(),
            d.media.clone(),
            d.store.clone(),
        )
    }),
    ("AliExpress reads", Kind::Sharded, |d| {
        crate::aliexpress::subscription::reads_subscription(
            d.aliexpress.clone(),
            d.db.clone(),
            d.store.clone(),
        )
    }),
    ("AliExpress pushes", Kind::Sharded, |d| {
        crate::aliexpress::webhook::pushes_subscription(
            d.aliexpress.clone(),
            d.db.clone(),
            d.store.clone(),
        )
    }),
    ("pictures", Kind::PerNode, |d| {
        crate::media::subscription(d.media.clone())
    }),
    ("shopper keys", Kind::PerNode, |d| {
        crate::shopper_keys::subscription(d.keys.clone())
    }),
    // Nor this: it erases the newsletter subscription a deleted account
    // names.
    ("shopper erasure", Kind::Sharded, |d| {
        crate::shopper_erasure::subscription(d.store.clone(), d.keys.ring())
    }),
    ("message keys", Kind::PerNode, |d| {
        crate::message_keys::subscription(d.keys.clone())
    }),
    ("newsletter keys", Kind::PerNode, |d| {
        crate::newsletter_keys::subscription(d.keys.clone())
    }),
    ("payments", Kind::Sharded, |d| {
        crate::payments::subscription(d.checkouts.clone())
    }),
    ("order payments", Kind::Sharded, |d| {
        crate::payments::order_payments_subscription(
            d.payments.clone(),
            d.store.clone(),
            d.db.clone(),
        )
    }),
    ("discount usage", Kind::Sharded, |d| {
        timada_orders::discount_usage_subscription(d.store.clone())
    }),
    ("stock usage", Kind::Sharded, |d| {
        timada_orders::stock_usage_subscription(d.store.clone())
    }),
    ("catalog releases", Kind::Sharded, |d| {
        timada_catalog::release_subscription(d.store.clone())
    }),
    ("discount releases", Kind::Sharded, |d| {
        timada_discounts::release_subscription(d.store.clone())
    }),
    ("page releases", Kind::Sharded, |d| {
        timada_pages::release_subscription(d.store.clone())
    }),
    ("newsletter opt-ins", Kind::Sharded, |d| {
        crate::newsletter::subscription(d.keys.clone(), d.store.clone())
    }),
    ("automation triggers", Kind::Sharded, |d| {
        crate::automations::triggers::subscription(d.db.clone(), d.store.clone())
    }),
    ("invoices", Kind::Sharded, |d| {
        crate::invoicing::subscription(
            d.store.clone(),
            d.vies.clone(),
            d.euro_rates.clone(),
            d.store.clone(),
            d.db.clone(),
        )
    }),
    // The mails a command owes are handed to the mailroom from here, after
    // the intent is recorded, and sent off the cursor: the sign-in codes of
    // the platform and of the shops, the invitations, the newsletter
    // confirmations, the shops' mails about their orders, the notices of a
    // message's arrival.
    ("platform code mails", Kind::Sharded, |d| {
        crate::code_mails::platform_subscription(d.mailroom.clone())
    }),
    ("shopper code mails", Kind::Sharded, |d| {
        crate::code_mails::shopper_subscription(d.mailroom.clone())
    }),
    ("invitation mails", Kind::Sharded, |d| {
        crate::invitation_mails::subscription(d.mailroom.clone())
    }),
    ("newsletter confirmations", Kind::Sharded, |d| {
        crate::newsletter_confirmations::subscription(d.mailroom.clone())
    }),
    ("order mails", Kind::Sharded, |d| {
        crate::order_mails::subscription(d.mailroom.clone())
    }),
    ("message notices", Kind::Sharded, |d| {
        crate::message_notifications::subscription(d.mailroom.clone())
    }),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    fn server(region: Option<&str>, also_serves: &[&str]) -> ServerConfig {
        ServerConfig {
            url: "https://timada.localhost".into(),
            host: "127.0.0.1".into(),
            port: 3000,
            region: region.map(str::to_owned),
            also_serves: also_serves.iter().map(|s| s.to_string()).collect(),
            trusted_proxies: Vec::new(),
        }
    }

    fn names(server: &ServerConfig) -> Vec<String> {
        planned(server)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect()
    }

    /// Without a region, or without other regions to serve, the table as it
    /// is; with them, every saga again per region, no read model.
    #[test]
    fn a_saga_is_planned_once_per_region_served() {
        let plain = names(&server(None, &[]));
        assert_eq!(plain.len(), SUBSCRIPTIONS.len());
        assert_eq!(names(&server(Some("eu-west-1"), &[])), plain);

        let more = names(&server(Some("eu-west-1"), &["us-east-3", "ap-south-1"]));
        let sagas = SUBSCRIPTIONS
            .iter()
            .filter(|(_, kind, _)| *kind == Kind::Sharded)
            .count();
        assert_eq!(more.len(), SUBSCRIPTIONS.len() + 2 * sagas);
        assert!(more.contains(&"payments for us-east-3".to_owned()));
        assert!(more.contains(&"stock usage for ap-south-1".to_owned()));
        assert!(!more.iter().any(|name| name.starts_with("catalog for")));
        assert!(!more.iter().any(|name| name.starts_with("pictures for")));
        // The copy follows the one in the table, so the page lists them
        // together.
        let at = more.iter().position(|n| n == "payments").unwrap();
        assert_eq!(more[at + 1], "payments for us-east-3");
    }

    /// Every subscription in the table is classified on purpose: the read
    /// models and the per-node effects everywhere, the rest sharded.
    #[test]
    fn read_models_follow_every_region() {
        for (name, kind, _) in SUBSCRIPTIONS {
            let read_model = [
                "identity",
                "tenancy",
                "members history",
                "AliExpress access",
                "catalog",
                "orders",
                "shoppers",
                "discounts",
                "reviews",
                "support",
                "newsletter",
                "automations",
                "pages",
                "invoicing",
                "AliExpress",
            ]
            .contains(name);
            let per_node = [
                "mail logos",
                "pictures",
                "shopper keys",
                "message keys",
                "newsletter keys",
            ]
            .contains(name);
            assert_eq!(*kind == Kind::ReadModel, read_model, "{name} is {kind:?}");
            assert_eq!(*kind == Kind::PerNode, per_node, "{name} is {kind:?}");
        }
    }
}
