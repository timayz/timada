//! Mails the shopper of every order, under the shop's name, with a link to
//! the order on the shop's storefront when the merchant said where it
//! lives: its confirmation once placed, the payment taken once its invoice
//! is issued (so the page it links to shows it), its shipment with the
//! parcel number and where to follow it, and its cancellation, saying
//! whether money goes back.
//!
//! The order's own events owe the mails; this subscription sees them on
//! every node and hands each to the node's [`Mailroom`], which sends off the
//! cursor, a lane per shop, so a shop whose server does not answer holds up
//! its own mails and nobody else's (#482). The node that writes the claim on
//! the mail's stream first is the one that sends
//! ([`timada_core::Sending`], `OrderMail`), then records how it went: a
//! server that refuses the address for good leaves `MailUndeliverable`,
//! which the order's page shows; one that does not answer leaves
//! `MailDeferred`, and whichever node looks once the retry is due sends
//! again, until the retries run out or the mail's day is over (`MailFailed`,
//! shown too). A pass never fails for a mail server.
//!
//! A shop with no mail server of its own sends through the platform's. That
//! server mails about the orders the checkout placed and no other: an order
//! a `store:write` key forged in the shop's store would have the platform
//! mail any address, so one whose payment the provider does not know as the
//! shop's is passed over (timada #431). A shop's own server is the shop's
//! to spend.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::Sending;
use timada_invoicing::aggregator::book::InvoiceIssued;
use timada_orders::ORDER_MAIL_WINDOW_SECS;
use timada_orders::aggregator::{Cancelled, Fulfilled, OrderMail, Placed};
use timada_tenancy::ShopMailKind;
use topcoat::context::{Cx, app_context};

use crate::code_mails::claimant;
use crate::mail::{
    send_order_cancelled, send_order_confirmation, send_order_paid, send_order_shipped,
};
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::payments::OrderPayment;
use crate::shop_mail::ShopMail;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/order-mails";

/// Follows every tenant's orders and invoices into the mailroom's table,
/// and the mails' own streams back into it. Every node runs it; the node
/// that claims an order's mail first is the one that sends it.
pub fn subscription<E: Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(room)
        .handler(on_placed())
        .handler(on_invoice_issued())
        .handler(on_fulfilled())
        .handler(on_cancelled())
        .handler(on_order_mail_event())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

/// Writes that the event at `at` (unix seconds) owes the shopper of
/// `order_id` the mail `kind`.
async fn owe<E: Executor>(
    context: &Context<'_, E>,
    tenant: &str,
    order_id: &str,
    kind: ShopMailKind,
    version: u16,
    at: u64,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    room.record(
        Kind::OrderMail(kind),
        tenant,
        order_id,
        Change::Owed {
            version,
            expires_at: at + ORDER_MAIL_WINDOW_SECS,
        },
        crate::auth::now(),
    )
    .await
}

#[evento::subscription]
async fn on_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Placed>,
) -> anyhow::Result<()> {
    owe(
        context,
        &event.tenant,
        &event.aggregate_id,
        ShopMailKind::OrderConfirmation,
        event.version,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn on_invoice_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<InvoiceIssued>,
) -> anyhow::Result<()> {
    owe(
        context,
        &event.tenant,
        &event.data.order_id,
        ShopMailKind::OrderPaid,
        event.version,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn on_fulfilled<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Fulfilled>,
) -> anyhow::Result<()> {
    owe(
        context,
        &event.tenant,
        &event.aggregate_id,
        ShopMailKind::OrderShipped,
        event.version,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn on_cancelled<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Cancelled>,
) -> anyhow::Result<()> {
    owe(
        context,
        &event.tenant,
        &event.aggregate_id,
        ShopMailKind::OrderCancelled,
        event.version,
        event.timestamp,
    )
    .await
}

/// Follows a mail's own stream into the table: whichever of its events this
/// is, the row is written from where the stream stands now, which names the
/// order and the mail the row is keyed by. A pass that handles several of
/// its events writes the same row each time.
#[evento::subscription_all]
async fn on_order_mail_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<OrderMail>,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    let Some(mail) = timada_orders::order_mail_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let Some(kind) = Kind::parse(&mail.mail) else {
        tracing::error!(tenant = %event.tenant, order = %mail.order_id, mail = %mail.mail, "not a mail about an order; left alone");
        return Ok(());
    };
    let Some(change) = change_of(&mail.sending) else {
        return Ok(());
    };
    room.record(
        kind,
        &event.tenant,
        &mail.order_id,
        change,
        crate::auth::now(),
    )
    .await
}

/// What the table records of a mail's stream in state `sending`: nothing
/// while no node took it (the intent's row stands as it is). The tries are
/// the stream's own count (#521).
pub(crate) fn change_of(sending: &Sending) -> Option<Change> {
    Some(match sending {
        Sending::Pending => return None,
        Sending::Claimed { until, .. } => Change::Claimed { until: *until },
        Sending::Deferred {
            retry_at,
            attempts,
            reason,
        } => Change::Deferred {
            retry_at: *retry_at,
            attempts: *attempts,
            reason: reason.clone(),
        },
        Sending::Failed { reason, attempts } => Change::Failed {
            reason: reason.clone(),
            attempts: *attempts,
        },
        Sending::Sent | Sending::Undeliverable { .. } => Change::Settled,
    })
}

/// Sends the mail `kind` about the order `owed` names at `now` (unix
/// seconds), from the mailroom: checks it is the shopper's to get, claims
/// it, mails it, records how it went.
pub(crate) async fn send(
    room: &Mailroom,
    owed: &OwedMail,
    kind: ShopMailKind,
    now: u64,
) -> anyhow::Result<Outcome> {
    let tenant_id = owed.tenant_id.as_str();
    let order_id = owed.stream_id.as_str();
    let orders = timada_orders::Command::new(room.store.clone(), tenant_id);
    // Read whole: what is written with the event (a discount, the shipping)
    // is in the same commit.
    let Some(order) = orders.load_order(order_id).await? else {
        return Ok(Outcome::Left);
    };
    // Money the merchant collected outside timada was never taken from the
    // card: the two settled it between them, and the mail would say
    // otherwise.
    if kind == ShopMailKind::OrderPaid && order.settled_outside.is_some() {
        return Ok(Outcome::Dropped);
    }
    let Some(shop) = timada_tenancy::Command(room.store.clone())
        .load_tenant(tenant_id)
        .await?
    else {
        return Ok(Outcome::Left);
    };
    let cx = Cx::new(room.mail.clone());
    // The platform's server speaks for the orders the checkout placed alone.
    // A provider that does not answer is asked again at the next look; one
    // that knows no such payment settles it: the shopper has the order's
    // page.
    if !app_context::<ShopMail>(&cx).has_shop_mails_server(&shop) {
        match room.payments.order_payment(&shop, &order).await {
            Ok(OrderPayment::Held | OrderPayment::Gone) => {}
            Ok(OrderPayment::Unknown) => {
                tracing::warn!(tenant = %tenant_id, order = %order.id, mail = kind.key(), "order mail not sent: the provider knows no payment of this order, and the platform's server would send it");
                return Ok(Outcome::Dropped);
            }
            Err(err) => {
                tracing::warn!(tenant = %tenant_id, order = %order.id, mail = kind.key(), error = %format!("{err:#}"), "order mail not sent yet: the payment provider did not answer, and the platform's server would send it");
                return Ok(Outcome::Left);
            }
        }
    }
    let link = orders
        .storefront_address()
        .await?
        .map(|address| format!("{address}/orders/{}", order.id));
    if !orders
        .claim_order_mail(
            &order.id,
            kind.key(),
            owed.expires_at.max(0) as u64,
            claimant(room, now),
        )
        .await?
    {
        return Ok(Outcome::Left);
    }
    let link = link.as_deref();
    let sent = attempt(async {
        match kind {
            ShopMailKind::OrderPaid => send_order_paid(&cx, &shop, &order, link).await,
            ShopMailKind::OrderShipped => send_order_shipped(&cx, &shop, &order, link).await,
            ShopMailKind::OrderCancelled => send_order_cancelled(&cx, &shop, &order, link).await,
            _ => send_order_confirmation(&cx, &shop, &order, link).await,
        }
    })
    .await;
    match sent {
        Attempt::Sent => {
            tracing::info!(tenant = %tenant_id, order = %order.id, mail = kind.key(), "order mail sent");
            orders.record_order_mail_sent(&order.id, kind.key()).await?;
            Ok(Outcome::Sent)
        }
        Attempt::Undeliverable(reason) => {
            tracing::warn!(tenant = %tenant_id, order = %order.id, mail = kind.key(), %reason, "order mail not deliverable");
            orders
                .record_order_mail_undeliverable(&order.id, kind.key(), &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        Attempt::NotSent(reason) => {
            tracing::warn!(tenant = %tenant_id, order = %order.id, mail = kind.key(), %reason, "order mail not sent; deferred");
            orders
                .record_order_mail_not_sent(&order.id, kind.key(), &room.node, &reason, now)
                .await?;
            Ok(Outcome::Deferred)
        }
    }
}

#[cfg(test)]
mod tests {
    use evento::{Evento, Turso};
    use timada_core::{MailStatus, RETRY_DELAYS_SECS, Sending};
    use timada_orders::{PROVIDER_STRIPE, Payment, ShippingAddress};
    use timada_tenancy::{ConfigurePayments, CreateTenant, TenantState};
    use topcoat::mail::{Mailbox, MemoryTransport, Transport};

    use super::*;
    use crate::code_mails::tests::{mail_context, vault};
    use crate::mail_fault::fake_smtp;
    use crate::mailroom::tests::{room_paid_through, rows, store};
    use crate::payments::fake::FakeGateway;

    const ORDER: &str = "order-1";
    const CART: &str = "cart-1";
    const PAYMENT: &str = "pi_1";

    /// A shop paid through `gateway`, with an order placed for a payment the
    /// gateway holds; the shop.
    async fn shop_with_order(store: &Turso, room: &Mailroom, gateway: &FakeGateway) -> TenantState {
        let evento = Evento::new(store.clone());
        let tenancy = timada_tenancy::Command(evento.clone());
        let shop = tenancy
            .create_tenant(CreateTenant {
                name: "Acme Tea".to_owned(),
                owner_user_id: "user-alice".to_owned(),
            })
            .await
            .unwrap();
        tenancy
            .configure_payments(
                "user-alice",
                &shop.id,
                ConfigurePayments {
                    provider: PROVIDER_STRIPE.into(),
                    publishable_key: "pk_test_123".into(),
                    secret_key_sealed: room.payments.seal(&shop.id, "sk_test_good").unwrap(),
                    webhook_secret_sealed: room.payments.seal(&shop.id, "whsec_test").unwrap(),
                },
            )
            .await
            .unwrap();
        gateway.hold(PAYMENT, &shop.id, CART, 1000);
        evento::append(ORDER)
            .tenant(&shop.id)
            .original_version(0)
            .event(&Placed {
                reference: "K7Q4-MZP2".to_owned(),
                cart_id: CART.to_owned(),
                email: "ada@example.com".to_owned(),
                shipping: ShippingAddress::default(),
                note: None,
                lines: Vec::new(),
                currency: "EUR".to_owned(),
                total_cents: 1000,
                payment: Payment {
                    provider: PROVIDER_STRIPE.to_owned(),
                    payment_id: PAYMENT.to_owned(),
                    amount_cents: 1000,
                    currency: "EUR".to_owned(),
                    capture_before: None,
                },
                supplier: None,
                placed_by: "storefront".to_owned(),
            })
            .commit(&evento)
            .await
            .unwrap();
        tenancy.load_tenant(&shop.id).await.unwrap().unwrap()
    }

    /// A mailroom for `node` over `store`, mailing through `transport` and
    /// asking `gateway` about the payments.
    fn node(
        store: &Turso,
        transport: impl Transport + 'static,
        gateway: &Arc<FakeGateway>,
        node: &str,
    ) -> Arc<Mailroom> {
        room_paid_through(
            store,
            mail_context(transport),
            vault(),
            gateway.clone(),
            node,
        )
    }

    async fn pass(store: &Turso, room: &Arc<Mailroom>) {
        subscription::<Turso>(room.clone())
            .ephemeral()
            .no_retry()
            .run_once(store)
            .await
            .expect("a pass never fails for a mail");
    }

    /// A pass and a sweep at `now`: what a node does with a new intent.
    async fn run(store: &Turso, room: &Arc<Mailroom>, now: u64) -> usize {
        pass(store, room).await;
        // The pass stamped the rows with the clock, which may have ticked
        // past the test's `now` meanwhile: sweep at whichever is later. A
        // failed send is stamped with that later time too, so a test reads
        // its retry off the stream rather than reckon it from `now`.
        room.sweep(now.max(crate::auth::now())).await.unwrap()
    }

    async fn mail_of(store: &Turso, shop: &TenantState, kind: ShopMailKind) -> Sending {
        timada_orders::Command::new(Evento::new(store.clone()), &shop.id)
            .load_order_mail(ORDER, kind.key())
            .await
            .unwrap()
            .map(|mail| mail.sending)
            .unwrap_or_default()
    }

    /// The order's mails as the merchant's page reads them.
    async fn shown(store: &Turso, shop: &TenantState) -> Vec<(String, MailStatus)> {
        let db = crate::db::Db::from(store.clone());
        timada_orders::read_model_subscription::<Turso>(db.clone())
            .ephemeral()
            .run_once(store)
            .await
            .unwrap();
        timada_orders::order_mails(&db, &shop.id, ORDER)
            .await
            .unwrap()
            .into_iter()
            .map(|mail| (mail.mail.clone(), mail.status()))
            .collect()
    }

    /// Two nodes see the order placed; one mails its confirmation, under
    /// the shop's name, and the other finds it taken.
    #[tokio::test]
    async fn an_order_confirmation_is_mailed_once_however_many_nodes_see_it() {
        let store = store().await;
        let gateway = FakeGateway::new();
        let transport = MemoryTransport::new();
        let node_a = node(&store, transport.clone(), &gateway, "node-a");
        let node_b = node(&store, transport.clone(), &gateway, "node-b");
        let shop = shop_with_order(&store, &node_a, &gateway).await;
        let now = crate::auth::now();

        assert_eq!(run(&store, &node_a, now).await, 1);
        assert_eq!(run(&store, &node_b, now).await, 0);
        assert_eq!(run(&store, &node_a, now).await, 0);

        let mails = transport.sent();
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].to(), [Mailbox::new("ada@example.com").unwrap()]);
        assert_eq!(mails[0].subject(), "Your Acme Tea order K7Q4-MZP2");
        assert_eq!(
            mail_of(&store, &shop, ShopMailKind::OrderConfirmation).await,
            Sending::Sent
        );
        assert_eq!(
            shown(&store, &shop).await,
            [("order_confirmation".to_owned(), MailStatus::Sent)]
        );
        // The passes after the send see it settled on the mail's stream.
        pass(&store, &node_a).await;
        pass(&store, &node_b).await;
        assert!(rows(&store).await.is_empty());
    }

    /// A shop's server that does not answer defers the mail: the merchant's
    /// page says so, and the retry, due later, goes out from whichever node
    /// looks.
    #[tokio::test]
    async fn a_server_that_does_not_answer_defers_the_mail_until_the_retry() {
        let store = store().await;
        let gateway = FakeGateway::new();
        let down = node(
            &store,
            fake_smtp::transport(fake_smtp::closed_port()),
            &gateway,
            "node-a",
        );
        let shop = shop_with_order(&store, &down, &gateway).await;
        let now = crate::auth::now();

        assert_eq!(run(&store, &down, now).await, 0);
        let mail = mail_of(&store, &shop, ShopMailKind::OrderConfirmation).await;
        let Sending::Deferred {
            retry_at,
            attempts: 1,
            ..
        } = mail
        else {
            panic!("{mail:?}");
        };
        assert_eq!(
            shown(&store, &shop).await,
            [("order_confirmation".to_owned(), MailStatus::Retrying)]
        );
        // Not before the retry is due.
        assert_eq!(run(&store, &down, retry_at - 1).await, 0);
        assert_eq!(
            mail_of(&store, &shop, ShopMailKind::OrderConfirmation)
                .await
                .attempts(),
            1
        );
        // The pass handled the three events of the mail's stream: the table
        // counts the one try they record (#521).
        assert_eq!(rows(&store).await[0].attempts, 1);

        let transport = MemoryTransport::new();
        let back = node(&store, transport.clone(), &gateway, "node-b");
        assert_eq!(run(&store, &back, retry_at).await, 1);
        assert_eq!(transport.sent().len(), 1);
        assert_eq!(
            mail_of(&store, &shop, ShopMailKind::OrderConfirmation).await,
            Sending::Sent
        );
    }

    /// A shop's server down for good: every try of the schedule fails, and
    /// the last gives the mail up. The cluster page counts the tries the
    /// mail's stream made, however many of its events each pass handled
    /// (#521).
    #[tokio::test]
    async fn a_server_down_for_good_gives_the_mail_up_after_the_schedule() {
        let store = store().await;
        let gateway = FakeGateway::new();
        let down = node(
            &store,
            fake_smtp::transport(fake_smtp::closed_port()),
            &gateway,
            "node-a",
        );
        let shop = shop_with_order(&store, &down, &gateway).await;

        assert_eq!(run(&store, &down, crate::auth::now()).await, 0);
        for tried in 1..=RETRY_DELAYS_SECS.len() {
            let mail = mail_of(&store, &shop, ShopMailKind::OrderConfirmation).await;
            let Sending::Deferred { retry_at, .. } = mail else {
                panic!("try {tried}: {mail:?}");
            };
            assert_eq!(run(&store, &down, retry_at).await, 0);
        }
        let tries = RETRY_DELAYS_SECS.len() as u32 + 1;
        let mail = mail_of(&store, &shop, ShopMailKind::OrderConfirmation).await;
        assert!(
            matches!(&mail, Sending::Failed { attempts, .. } if *attempts == tries),
            "{mail:?}"
        );
        pass(&store, &down).await;
        let owed = rows(&store).await;
        assert_eq!(owed.len(), 1);
        assert!(owed[0].failed);
        assert_eq!(owed[0].attempts, i64::from(tries));
    }

    /// The platform's server speaks for the orders the checkout placed
    /// alone: a payment the provider does not know drops the mail, a
    /// provider that does not answer leaves it for the next look. A payment
    /// mail of an order settled outside is nobody's to get.
    #[tokio::test]
    async fn the_platforms_server_mails_about_the_orders_the_provider_knows() {
        let store = store().await;
        let gateway = FakeGateway::new();
        let transport = MemoryTransport::new();
        let room = node(&store, transport.clone(), &gateway, "node-a");
        let shop = shop_with_order(&store, &room, &gateway).await;
        let now = crate::auth::now();

        // The provider is away: the mail waits, unclaimed.
        gateway.set_down(Some("stripe is away"));
        assert_eq!(run(&store, &room, now).await, 0);
        assert_eq!(
            mail_of(&store, &shop, ShopMailKind::OrderConfirmation).await,
            Sending::Pending
        );
        let waiting = rows(&store).await;
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].due_at > now as i64, "put off to the next look");
        // Back, and the payment known: sent.
        gateway.set_down(None);
        assert_eq!(room.sweep(waiting[0].due_at as u64).await.unwrap(), 1);
        assert_eq!(transport.sent().len(), 1);

        // A mail about an order whose payment the provider never saw.
        let orders = timada_orders::Command::new(Evento::new(store.clone()), &shop.id);
        evento::append("order-forged")
            .tenant(&shop.id)
            .original_version(0)
            .event(&Placed {
                reference: "FORGED-1".to_owned(),
                cart_id: "cart-forged".to_owned(),
                email: "anyone@example.com".to_owned(),
                shipping: ShippingAddress::default(),
                note: None,
                lines: Vec::new(),
                currency: "EUR".to_owned(),
                total_cents: 1000,
                payment: Payment {
                    provider: PROVIDER_STRIPE.to_owned(),
                    payment_id: "pi_forged".to_owned(),
                    amount_cents: 1000,
                    currency: "EUR".to_owned(),
                    capture_before: None,
                },
                supplier: None,
                placed_by: "store:write".to_owned(),
            })
            .commit(&Evento::new(store.clone()))
            .await
            .unwrap();
        assert_eq!(run(&store, &room, now).await, 0);
        assert_eq!(transport.sent().len(), 1, "nothing more went out");
        assert!(
            orders
                .load_order_mail("order-forged", "order_confirmation")
                .await
                .unwrap()
                .is_none(),
            "never claimed"
        );
        assert!(rows(&store).await.is_empty(), "dropped");

        // The money was settled outside: no payment mail.
        evento::append(ORDER)
            .tenant(&shop.id)
            .original_version(1)
            .event(&timada_orders::aggregator::SettledOutside {
                note: None,
                settled_by: "user-alice".to_owned(),
            })
            .commit(&Evento::new(store.clone()))
            .await
            .unwrap();
        room.record(
            Kind::OrderMail(ShopMailKind::OrderPaid),
            &shop.id,
            ORDER,
            Change::Owed {
                version: 2,
                expires_at: now + ORDER_MAIL_WINDOW_SECS,
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(room.sweep(now).await.unwrap(), 0);
        assert_eq!(transport.sent().len(), 1);
        assert!(rows(&store).await.is_empty());
        assert_eq!(
            mail_of(&store, &shop, ShopMailKind::OrderPaid).await,
            Sending::Pending
        );
    }
}
