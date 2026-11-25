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
    // The node's certificate for the other nodes: the store's port, the
    // requests this node makes of the others and the port it answers them on
    // present it, as the node reads it again once renewed (#726).
    let tls = cluster::node_tls(config.cluster.as_ref())?;
    // The store twice over the same node: as the node reads it (behind a
    // read barrier with peers), for the pages; and from this node's file as
    // it stands, for the subscriptions and the storefronts (#466).
    let (cluster, executor, local) = cluster::open(
        config.cluster.as_ref(),
        tls.clone(),
        config.server.work_group.as_deref(),
        &config.server.also_works_for,
        config.server.also_works_for_ungrouped,
        &config.database.path,
        store.clone(),
    )
    .await?;
    // The read models in the store's database.
    let db = db::Db::from(store.clone());

    let peers =
        crate::peers::Peers::from_cluster(config.cluster.as_ref(), tls.as_ref())?.map(Arc::new);
    let media = Arc::new(crate::media::Media::new(&config.media, peers.clone()));
    // The pictures this node lacks are fetched off the subscriptions'
    // cursors (#468).
    let fetcher = crate::media::Fetcher::new(media.clone(), db.clone());
    // How much disk a tenant's pictures may take.
    let quota = Arc::new(crate::media::Quota::new(
        db.clone(),
        config.media.default_quota(),
    ));
    let keys_sealer = crate::seal::keys_sealer(&config);
    if keys_sealer.is_none() {
        tracing::warn!(
            "secrets.key is not set: no shopper can create an account and no contact message can be received"
        );
    }
    // Where the studies' keywords come from, for the pages and the saga
    // that fetches them.
    let keywords = crate::keywords::Keywords::new(&config.dataforseo);
    if !keywords.enabled() {
        tracing::info!("dataforseo.login is not set: keyword research is off");
    }
    // The model that drafts the shops' texts, for the pages and the saga
    // that asks it.
    let writers = crate::writer::Writers::new(&config.writer);
    if !writers.enabled() {
        tracing::info!("writer.key is not set: text drafts are off");
    }
    let writer_spending = crate::writer::Spending::new(&config.writer);
    // The application the shops connect their Google accounts to.
    let google = crate::google::Google::new(crate::seal::google_sealer(&config), &config.google);
    if !google.enabled() {
        tracing::info!(
            "google.client_id or secrets.key is not set: no shop connects a Google account"
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
    // What the other nodes ask of this one, on the port they keep for it,
    // from now on: they copy their pictures and keys here, and ask the
    // limits, while this node catches up. Validated by `Config::load`.
    let listen = config
        .cluster
        .as_ref()
        .and_then(crate::config::ClusterConfig::web_listen_addr);
    let peer_server = match (tls, listen) {
        (Some(tls), Some(listen)) => Some(
            crate::peer_port::serve(
                listen,
                tls,
                cluster.clone(),
                media.clone(),
                keys.clone(),
                limits.clone(),
            )
            .await?,
        ),
        _ => None,
    };
    let gateway = Arc::new(crate::payments::StripeGateway::new(
        config
            .payments
            .as_ref()
            .and_then(|payments| payments.stripe_api_url.clone()),
    ));
    let payments = Arc::new(crate::payments::Payments::new(
        crate::seal::payments_sealer(&config),
        gateway,
    ));
    // The shops' own mail servers, shared by the pages and the mails sent
    // in the background so their connections are too.
    let shop_mail = crate::shop_mail::ShopMail::new(
        crate::seal::mail_sealer(&config),
        config.mail.allow_private_hosts,
    );
    // This node's place among the nodes up, for the work they share out.
    let place = cluster::share_of(&cluster);
    // The euro's rates, read from the European Central Bank: for the sales,
    // invoices and imports in other currencies.
    let euro_rates: Arc<dyn crate::ecb::EuroRates> = Arc::new(crate::ecb::EcbClient::new(
        config.invoicing.euro_rates_url.clone(),
    ));
    let aliexpress = crate::aliexpress::Aliexpress::new(
        crate::seal::aliexpress_sealer(&config),
        &config.aliexpress,
        config.mail.allow_private_hosts,
        quota.clone(),
        place.clone(),
        euro_rates.clone(),
    )?;
    // The order confirmations and the notices of contact messages are
    // mailed from the subscriptions, outside any request: they get their
    // own transport.
    let background_mail = {
        let mailer = crate::mail::build(&config.mail)?;
        let mut app = topcoat::context::ContextMap::new();
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
            store_api::closing_subscription(connections.clone()),
            &local,
        )
        .await?,
    ];
    let store_api = store_api::serve(
        config.store_api.as_ref(),
        executor.clone(),
        local.clone(),
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
    // event that failed, and the cluster page says so. Each reads from this
    // node's file (`local`): a handler reads streams already behind the event
    // it handles, which the node has applied before handing the event over,
    // and a write it makes on a stale read is refused for its version, the
    // write it missed then applied here for the retry. After this start
    // copied the store from a peer, the read models are empty: each
    // subscription first catches up over the copied events, so the pages
    // never show an empty shop, and the router takes requests afterwards.
    // The mails the sagas owe go out from here, off the subscriptions'
    // cursors, a lane per shop: one shop's mail server never holds up the
    // others' codes, invitations and confirmations.
    let mailroom = crate::mailroom::Mailroom::new(
        db.clone(),
        local.0.clone(),
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
            local.0.clone(),
        )),
        local.0.clone(),
        cluster::node_name(&cluster),
    );
    let deps = Arc::new(Deps {
        db: db.clone(),
        store: local.0.clone(),
        mailroom: mailroom.clone(),
        checkouts: checkouts.clone(),
        media: media.clone(),
        fetcher: fetcher.clone(),
        keys: keys.clone(),
        payments: payments.clone(),
        aliexpress: aliexpress.clone(),
        keywords: keywords.clone(),
        spending: crate::research::Spending::new(&config.research),
        writers: writers.clone(),
        writer_spending,
        place: place.clone(),
        vies: Arc::new(crate::vies::ViesClient::new(
            config.invoicing.vies_url.clone(),
        )),
        euro_rates: euro_rates.clone(),
        conversions: Arc::new(crate::meta_conversions::Conversions::new(
            crate::seal::meta_sealer(&config),
            Arc::new(crate::meta_conversions::MetaClient::new(
                config.advertising.meta_api_url.clone(),
            )),
        )),
        google: google.clone(),
    });
    let planned = planned(&config.server);
    let copied = cluster.health().joined_at_start;
    let rebuilt = cluster::catch_up_mark(&config.database.path).exists();
    if copied || rebuilt {
        if copied {
            tracing::info!(
                "the store was copied from a peer: the read models catch up before serving"
            );
            // The keys first, page by page from a peer: the read models open
            // each shopper's, subscriber's and message's data with its key,
            // and would otherwise ask the peers for each in turn. Whatever a
            // peer could not hand over is asked for that way.
            match keys.copy_from_peers().await {
                Ok(taken) => {
                    tracing::info!(keys = taken, "copied the personal-data keys from a peer")
                }
                Err(err) => tracing::warn!(
                    error = %format!("{err:#}"),
                    "the personal-data keys were not all copied from a peer: the read models ask for each one missing"
                ),
            }
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
            let local = local.clone();
            catching_up.spawn(async move {
                if let Err(err) = workers::catch_up(&name, builder, &local).await {
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
    // Those that fill the read models, apart: one of them in trouble is
    // what makes the node's pages stale.
    let mut read_model_workers = Vec::new();
    for (name, kind, make) in planned {
        let worker = workers::start(name, make(&deps), &local).await?;
        if kind == Kind::ReadModel {
            read_model_workers.push(worker.clone());
        }
        all_workers.push(worker);
    }
    let workers = Workers::new(all_workers);
    let read_model_workers = Workers::new(read_model_workers);
    // The sagas of a work group whose node is away are run from here, and handed
    // back when it returns (#709): a node with a work group of its own may be
    // its only one, and a stream keeps the work group it was created in. One of
    // the nodes up takes each such work group on, under a lease the cluster
    // gives, and goes on from where the node before it stopped. A work group no
    // node was heard from for a minute counts as without one: longer than a
    // node takes to restart for an update. The sagas of this node's own
    // work group stay with the workers above; a node without a work group takes
    // none on.
    let cover = config.server.work_group.is_some().then(|| {
        let mut cover = cluster.cover(local.0.clone());
        for (name, kind, make) in SUBSCRIPTIONS {
            if *kind == Kind::Sharded {
                let deps = deps.clone();
                let make = *make;
                // Supervised, as this node's own: one that fails goes on by
                // itself and the work group stays where it is.
                cover =
                    cover.subscription(*name, move |_work_group: &str| make(&deps).supervised());
            }
        }
        cover.start()
    });
    let mailroom_sends = mailroom.start();
    let picture_fetches = fetcher.clone().start();
    // This node's picture files against the store, once a day.
    let picture_reconcile = crate::media::Reconcile::new(media.clone(), fetcher.clone()).start(
        std::time::Duration::from_secs(config.media.reconcile_check_secs.max(60)),
    );
    let checkout_answers = checkouts.clone().start();
    // The events the sagas gave up on stay on the cluster page a week.
    let dead_letters = crate::saga_fault::start_forgetting(
        executor.clone(),
        db.clone(),
        crate::saga_fault::FORGET_EVERY,
    );
    // What the balancer asks before sending traffic here, and what an
    // operator scrapes.
    let readiness = Arc::new(crate::readiness::Readiness::new(
        cluster.clone(),
        workers.clone(),
        read_model_workers,
        connections.clone(),
        config.monitoring.ready_lag(),
    ));
    let metrics = match config.monitoring.metrics_listen {
        Some(listen) => {
            Some(crate::metrics::serve(listen, readiness.clone(), &config.database.path).await?)
        }
        None => None,
    };
    // `snapshot save` asks this server for a copy of the store (#593).
    let store_copies = crate::store_copy::serve(&config.database.path, store.clone())?;

    // The operator's alert for a node that stays away: this node nearing the
    // writes it may keep for it, or a node given up on.
    let cluster_watch = cluster.watch(std::time::Duration::from_secs(60));
    // The timers below read from this node's file as well (#533). Each acts
    // off the rows of the read models, which this node derived from that
    // file, so what it loads is never older than the row that sent it there.
    // What it loads may still be a moment behind a write another node took:
    // every write a timer makes is checked against the version it loaded, or
    // claims a stream of its own, so a write made on such a load is refused,
    // and the write it missed is applied here before the refusal, for the
    // retry or the next look to see.

    // A few days after an order left, its buyer is asked for a review.
    let review_requests = crate::review_requests::ReviewRequests::new(
        db.clone(),
        local.0.clone(),
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
        crate::payments::LapsedCheckouts::new(db.clone(), local.0.clone(), payments.clone())
            .start(payments_every);
    let pending_payments =
        crate::payments::PendingPayments::new(db.clone(), local.0.clone(), payments.clone())
            .start(payments_every);
    let unanswered_checkouts =
        crate::payments::UnansweredCheckouts::new(db.clone(), local.0.clone())
            .start(payments_every);
    // Two days before the hold on a shopper's card lapses on an order
    // AliExpress may have, the members who manage orders are reminded to
    // take the payment.
    let hold_reminders = crate::hold_reminders::HoldReminders::new(
        db.clone(),
        local.0.clone(),
        crate::hold_reminders::PlatformMail(background_mail.clone()),
        url.clone(),
    )
    .start(payments_every);

    // The exports an owner scheduled go out once their day, week or month
    // has ended on the shop's clock, each node sending its own part of the
    // shops'.
    let live = cluster.clone();
    let scheduled_exports = crate::scheduled_exports::ScheduledExports::new(
        db.clone(),
        local.0.clone(),
        Arc::new(move || {
            let (index, of) = cluster::live_place(&live, cluster::LIVE_WITHIN, cluster::Among::All);
            timada_core::Share { index, of }
        }),
        crate::scheduled_exports::Outbox::new(
            background_mail.clone(),
            config.mail.allow_private_hosts,
        )?,
        crate::seal::webhook_keys(&config),
        url.clone(),
    )
    .start(crate::scheduled_exports::SWEEP_EVERY);

    // What Search Console says of the connected shops' sites is read once a
    // day each, each node reading for its own part of the shops.
    let live = cluster.clone();
    let search_passes = crate::search_passes::SearchPasses::new(
        db.clone(),
        local.0.clone(),
        Arc::new(move || {
            let (index, of) = cluster::live_place(&live, cluster::LIVE_WITHIN, cluster::Among::All);
            timada_core::Share { index, of }
        }),
        google.clone(),
    )
    .start(std::time::Duration::from_secs(
        config.google.search_check_secs.max(1),
    ));

    // What Google Ads says the connected shops' accounts cost and brought
    // is read once a day each, the same way.
    let live = cluster.clone();
    let ads_passes = crate::ads_passes::AdsPasses::new(
        db.clone(),
        local.0.clone(),
        Arc::new(move || {
            let (index, of) = cluster::live_place(&live, cluster::LIVE_WITHIN, cluster::Among::All);
            timada_core::Share { index, of }
        }),
        google.clone(),
        config.google.ads_calls_a_day,
    )
    .start(std::time::Duration::from_secs(
        config.google.ads_check_secs.max(1),
    ));

    // And what Merchant Center says of their products.
    let live = cluster.clone();
    let merchant_passes = crate::merchant_passes::MerchantPasses::new(
        db.clone(),
        local.0.clone(),
        Arc::new(move || {
            let (index, of) = cluster::live_place(&live, cluster::LIVE_WITHIN, cluster::Among::All);
            timada_core::Share { index, of }
        }),
        google.clone(),
    )
    .start(std::time::Duration::from_secs(
        config.google.merchant_check_secs.max(1),
    ));

    // A cart nobody touched for `[carts] idle_days` is dropped, each node
    // dropping its own part of them.
    let live = cluster.clone();
    let idle_carts = crate::idle_carts::IdleCarts::new(
        db.clone(),
        local.0.clone(),
        Arc::new(move || {
            let (index, of) = cluster::live_place(&live, cluster::LIVE_WITHIN, cluster::Among::All);
            timada_core::Share { index, of }
        }),
        config.carts.idle(),
    )
    .start(std::time::Duration::from_secs(
        config.carts.check_secs.max(1),
    ));

    // Each shop's best sellers are ranked from its sales, for its
    // storefront to sort by, each node ranking its own part of the shops.
    let live = cluster.clone();
    let best_sellers = crate::best_sellers::BestSellers::new(
        db.clone(),
        local.0.clone(),
        Arc::new(move || {
            let (index, of) = cluster::live_place(&live, cluster::LIVE_WITHIN, cluster::Among::All);
            timada_core::Share { index, of }
        }),
        config.best_sellers.period(),
    )
    .start(std::time::Duration::from_secs(
        config.best_sellers.check_secs.max(1),
    ));

    // A campaign a merchant sent goes out from every node, each mailing its
    // own part of the subscribers, a lane per shop.
    let live = cluster.clone();
    let newsletter_sends = crate::newsletter_sends::NewsletterSends::new(
        db.clone(),
        local.0.clone(),
        background_mail.clone(),
        keys.ring(),
        Arc::new(move || {
            let (index, of) = cluster::live_place(&live, cluster::LIVE_WITHIN, cluster::Among::All);
            timada_core::Share { index, of }
        }),
        config.newsletter.mails_per_check,
        std::time::Duration::from_secs(config.newsletter.send_check_secs.max(1)),
    )
    .start();

    // The runs of the shops' automation scenarios move on a timer, each
    // node moving its own part of them.
    let automation_runs = crate::automations::runner::Runner::new(
        db.clone(),
        local.0.clone(),
        background_mail.clone(),
        url.clone(),
        keys.ring(),
        crate::seal::opt_out_key(&config),
        crate::seal::webhook_keys(&config),
        std::time::Duration::from_secs(config.automations.webhook_timeout_secs.max(1)),
        config.mail.allow_private_hosts,
        std::time::Duration::from_secs(config.automations.abandoned_after_secs.max(60)),
        config.automations.runs_per_check,
        std::time::Duration::from_secs(config.automations.check_secs.max(1)),
        place,
    )?
    .start();

    // The products imported from AliExpress are read again as the day's calls
    // allow, and the accounts and open orders on a timer.
    let aliexpress_sync = crate::aliexpress::sync::AliexpressSync::new(
        db.clone(),
        local.0.clone(),
        aliexpress.clone(),
        payments.clone(),
        std::time::Duration::from_secs(config.aliexpress.sync_check_secs.max(1)),
    )
    .start();

    let router = app::router(Services {
        executor,
        local,
        db,
        providers,
        webauthn,
        mailer,
        cluster: cluster.clone(),
        authorizer,
        media,
        quota: quota.clone(),
        keys,
        limits,
        email_codes: crate::seal::email_codes(&config),
        shopper_codes: crate::seal::shopper_codes(&config),
        invitation_tokens: crate::seal::invitation_tokens(&config),
        payments,
        shop_mail,
        aliexpress,
        keywords,
        writers,
        writer_spending,
        google,
        euro_rates,
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
    if let Some(peer_server) = peer_server {
        peer_server.shutdown().await;
    }
    store_copies.shutdown().await;
    cluster_watch.abort();
    if let Some(cover) = cover {
        cover.shutdown().await;
    }
    mailroom_sends.abort();
    picture_fetches.abort();
    picture_reconcile.abort();
    checkout_answers.abort();
    dead_letters.abort();
    review_requests.abort();
    lapsed_checkouts.abort();
    pending_payments.abort();
    unanswered_checkouts.abort();
    hold_reminders.abort();
    scheduled_exports.abort();
    search_passes.abort();
    ads_passes.abort();
    merchant_passes.abort();
    idle_carts.abort();
    best_sellers.abort();
    newsletter_sends.abort();
    automation_runs.abort();
    aliexpress_sync.abort();
    workers.shutdown().await;
    cluster.shutdown().await;
    drop(store);
    Ok(())
}

/// What the subscriptions are built from.
pub struct Deps {
    /// The read models in the store's database.
    pub db: db::Db,
    /// The store read from this node's file, which the sagas write back
    /// through.
    pub store: Executor,
    /// Where the mails the sagas owe are handed to be sent.
    pub mailroom: Arc<crate::mailroom::Mailroom>,
    /// Where the requests to pay are handed to be answered.
    pub checkouts: Arc<crate::payments::Checkouts>,
    pub media: Arc<crate::media::Media>,
    /// Where the pictures this node lacks are listed to be fetched.
    pub fetcher: Arc<crate::media::Fetcher>,
    pub keys: Arc<crate::key_vault::KeyVault>,
    pub payments: Arc<crate::payments::Payments>,
    pub aliexpress: Arc<crate::aliexpress::Aliexpress>,
    /// Where the studies' keywords are fetched from.
    pub keywords: crate::keywords::Keywords,
    /// What the fetches may spend in a day.
    pub spending: crate::research::Spending,
    /// The model that drafts the shops' texts.
    pub writers: crate::writer::Writers,
    /// What the drafts may spend in a day.
    pub writer_spending: crate::writer::Spending,
    /// This node's place among the nodes up: which of the work a saga is
    /// told about falls to it.
    pub place: cluster::ShareOf,
    pub vies: Arc<dyn crate::vies::Vies>,
    pub euro_rates: Arc<dyn crate::ecb::EuroRates>,
    /// What tells Meta of the sales of the shops that gave a token.
    pub conversions: Arc<crate::meta_conversions::Conversions>,
    /// Google, where the campaigns a member approved are created.
    pub google: crate::google::Google,
}

/// Builds one subscription from the parts.
type Make = fn(&Deps) -> SubscriptionBuilder<Executor>;

/// A worker built from a subscription: the one the table lists, or its copy
/// for a work group this node also serves, filtered on that work group's key, or
/// its copy for the events written without a work group, filtered on no key.
type Planned = (
    String,
    Kind,
    Box<dyn Fn(&Deps) -> SubscriptionBuilder<Executor> + Send + Sync>,
);

/// What a subscription follows, and what it is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Every work group's events, into the read-model tables. The builder says
    /// `.any_routing_key()`. After a start that copied the store from a
    /// peer, these catch up before the router takes requests.
    ReadModel,
    /// Every work group's events, for an effect each node has for itself: its
    /// copies of the keys. The builder says `.any_routing_key()`.
    PerNode,
    /// The node's own work group (the executor's default routing key, which the
    /// builder leaves alone), and each work group of `server.also_works_for` as a
    /// worker of its own, as are the events written without a work group under
    /// `server.also_works_for_ungrouped`: a saga, whose work a work group's nodes
    /// do for it.
    Sharded,
}

/// The workers to start for `server`: every subscription once, and the
/// sharded ones again for each work group this node also serves, named after it
/// (`payments for us-east-3`), and once more for the events written without
/// a work group when it serves those too (`payments for no work group`). That last
/// copy keeps its cursor under the saga's bare key, where this node kept it
/// while it had no work group, and writes what it creates in the node's work group.
/// Without a work group, one worker each, following the events written without
/// a key, as before.
fn planned(server: &crate::config::ServerConfig) -> Vec<Planned> {
    debug_assert!(
        (server.also_works_for.is_empty() && !server.also_works_for_ungrouped)
            || server.work_group.is_some(),
        "validated by Config::load"
    );
    let mut planned: Vec<Planned> = Vec::new();
    for (name, kind, make) in SUBSCRIPTIONS {
        planned.push((name.to_string(), *kind, Box::new(make)));
        if *kind == Kind::Sharded {
            for work_group in &server.also_works_for {
                let work_group = work_group.clone();
                planned.push((
                    format!("{name} for {work_group}"),
                    *kind,
                    Box::new(move |deps: &Deps| make(deps).routing_key(work_group.clone())),
                ));
            }
            if server.also_works_for_ungrouped {
                planned.push((
                    format!("{name} for no work group"),
                    *kind,
                    Box::new(move |deps: &Deps| make(deps).no_routing_key()),
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
/// it follows every work group or the node's own ([`Kind`]).
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
    ("mail logos", Kind::ReadModel, |d| {
        crate::media::logo_subscription(d.fetcher.clone())
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
    ("research", Kind::ReadModel, |d| {
        timada_research::read_model_subscription(d.db.clone())
    }),
    ("google", Kind::ReadModel, |d| {
        timada_google::read_model_subscription(d.db.clone())
    }),
    ("proposals", Kind::ReadModel, |d| {
        timada_proposals::read_model_subscription(d.db.clone())
    }),
    ("writer", Kind::ReadModel, |d| {
        timada_writer::read_model_subscription(d.db.clone())
    }),
    // The texts a member asked for are drafted by the writer and prepared
    // as proposals.
    ("text drafts", Kind::Sharded, |d| {
        crate::writer::drafts::subscription(
            d.writers.clone(),
            d.store.clone(),
            d.db.clone(),
            d.writer_spending,
        )
    }),
    // What a member approved is changed in the shop, under their name.
    ("proposal changes", Kind::Sharded, |d| {
        crate::proposals::apply::subscription(d.store.clone(), d.google.clone())
    }),
    // A paused campaign a member asked to run is set running at Google
    // Ads.
    ("ads campaigns", Kind::Sharded, |d| {
        crate::campaigns::subscription(d.store.clone(), d.google.clone())
    }),
    // A tuning round asked for, or due once Google Ads was read, reads the
    // campaign's last weeks and prepares its proposals.
    ("ads tuning", Kind::Sharded, |d| {
        crate::tuning::subscription(d.store.clone(), d.db.clone())
    }),
    // The keywords a study's owner asked for are fetched from the keyword
    // source and recorded on the study.
    ("keyword fetches", Kind::Sharded, |d| {
        crate::research::subscription(
            d.keywords.clone(),
            d.store.clone(),
            d.db.clone(),
            d.spending,
        )
    }),
    // Who holds the first results of a study's main keywords, searched one
    // keyword at a time.
    ("first results", Kind::Sharded, |d| {
        crate::research::results_subscription(d.keywords.clone(), d.store.clone(), d.spending)
    }),
    // The tree a study's owner asked for is created in a shop as draft
    // categories, the shop first when it is to be made.
    ("study categories", Kind::Sharded, |d| {
        crate::study_shop::subscription(d.store.clone())
    }),
    ("pictures", Kind::ReadModel, |d| {
        crate::media::subscription(d.fetcher.clone())
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
        crate::payments::order_payments_subscription(d.payments.clone(), d.store.clone())
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
        crate::automations::triggers::subscription(d.db.clone(), d.store.clone(), d.place.clone())
    }),
    ("invoices", Kind::Sharded, |d| {
        crate::invoicing::subscription(
            d.store.clone(),
            d.vies.clone(),
            d.euro_rates.clone(),
            d.store.clone(),
        )
    }),
    // The mails a command owes are handed to the mailroom from here, after
    // the intent is recorded, and sent off the cursor: the sign-in codes of
    // the platform and of the shops, the invitations (to join a shop, to
    // create an account), the newsletter
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
    ("account invitation mails", Kind::Sharded, |d| {
        crate::account_invitation_mails::subscription(d.mailroom.clone())
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
    ("proposal notices", Kind::Sharded, |d| {
        crate::proposals::notices::subscription(d.mailroom.clone())
    }),
    ("merchant notices", Kind::Sharded, |d| {
        crate::merchant_notices::subscription(d.mailroom.clone())
    }),
    // A shop's checkpoint is read when Google Ads was read and its day
    // came, and its owners are told.
    ("checkpoints", Kind::Sharded, |d| {
        crate::checkpoints::subscription(d.store.clone(), d.db.clone())
    }),
    ("checkpoint notices", Kind::Sharded, |d| {
        crate::checkpoints::notices_subscription(d.mailroom.clone())
    }),
    // The sales told to Meta from the server, for the shops that gave a
    // Conversions API token and the shoppers who allowed advertising.
    ("meta conversions", Kind::Sharded, |d| {
        crate::meta_conversions::subscription(d.conversions.clone(), d.store.clone())
    }),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    fn server(work_group: Option<&str>, also_works_for: &[&str]) -> ServerConfig {
        ServerConfig {
            url: "https://timada.localhost".into(),
            host: "127.0.0.1".into(),
            port: 3000,
            work_group: work_group.map(str::to_owned),
            also_works_for: also_works_for.iter().map(|s| s.to_string()).collect(),
            also_works_for_ungrouped: false,
            trusted_proxies: Vec::new(),
        }
    }

    fn names(server: &ServerConfig) -> Vec<String> {
        planned(server)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect()
    }

    /// Without a work group, or without other work groups to serve, the table as it
    /// is; with them, every saga again per work group, no read model.
    #[test]
    fn a_saga_is_planned_once_per_work_group_served() {
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

    /// A node that also serves the events written without a work group runs
    /// every saga once more for them, after its copies for other work groups.
    #[test]
    fn a_saga_is_planned_once_more_for_the_events_without_a_work_group() {
        let sagas = SUBSCRIPTIONS
            .iter()
            .filter(|(_, kind, _)| *kind == Kind::Sharded)
            .count();
        let keyless = names(&ServerConfig {
            also_works_for_ungrouped: true,
            ..server(Some("eu-west-1"), &["us-east-3"])
        });
        assert_eq!(keyless.len(), SUBSCRIPTIONS.len() + 2 * sagas);
        let at = keyless.iter().position(|n| n == "payments").unwrap();
        assert_eq!(keyless[at + 1], "payments for us-east-3");
        assert_eq!(keyless[at + 2], "payments for no work group");
        assert!(!keyless.iter().any(|n| n == "catalog for no work group"));
        assert!(
            !keyless
                .iter()
                .any(|n| n == "shopper keys for no work group")
        );
    }

    /// Every subscription in the table is classified on purpose: the read
    /// models and the per-node effects everywhere, the rest sharded.
    #[test]
    fn read_models_follow_every_work_group() {
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
                "research",
                "google",
                "proposals",
                "writer",
                "mail logos",
                "pictures",
            ]
            .contains(name);
            let per_node = ["shopper keys", "message keys", "newsletter keys"].contains(name);
            assert_eq!(*kind == Kind::ReadModel, read_model, "{name} is {kind:?}");
            assert_eq!(*kind == Kind::PerNode, per_node, "{name} is {kind:?}");
        }
    }
}
