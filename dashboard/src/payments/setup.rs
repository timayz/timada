//! Tells each tenant's streams how its shoppers pay, so a storefront can show
//! the payment form before it asks to pay.

use std::sync::Arc;

use evento::{
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_orders::PaymentSetup;
use timada_tenancy::TenantState;
use timada_tenancy::aggregator::{PaymentsConfigured, PaymentsRemoved, Resumed};

use super::{DASHBOARD_ACTOR, Payments};
use crate::cluster::Executor;

/// The cursor identity of [`setup_subscription`], in the platform store.
pub const SETUP_SUBSCRIPTION: &str = "timada-dashboard/payment-setup";

/// The store, to write each tenant's setup with (a handler only borrows the
/// executor it runs on).
#[derive(Clone)]
struct Store(Executor);

/// Follows the tenants' payment settings (the platform's streams) and copies
/// the public part (the provider and the publishable key) into each tenant's
/// own streams. Every node runs it; writing what is there already is a
/// no-op. A fresh node, or this subscription's first run, replays every
/// tenant's history, which brings the tenants set up before it in step.
pub fn setup_subscription(
    payments: Arc<Payments>,
    store: Executor,
) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::new(SETUP_SUBSCRIPTION)
        .data(payments)
        .data(Store(store))
        .handler(on_payments_configured())
        .handler(on_payments_removed())
        .handler(on_tenant_resumed())
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_payments_configured<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<PaymentsConfigured>,
) -> anyhow::Result<()> {
    sync(context, &event.aggregate_id).await
}

#[evento::subscription]
async fn on_payments_removed<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<PaymentsRemoved>,
) -> anyhow::Result<()> {
    sync(context, &event.aggregate_id).await
}

/// A suspended tenant could not be told: it is once it comes back.
#[evento::subscription]
async fn on_tenant_resumed<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<Resumed>,
) -> anyhow::Result<()> {
    sync(context, &event.aggregate_id).await
}

/// Writes what the tenant's settings say now, whichever event led here, so a
/// replay lands on the latest state.
async fn sync<E: evento::Executor>(
    context: &Context<'_, E>,
    tenant_id: &str,
) -> anyhow::Result<()> {
    let payments: Arc<Payments> = context.extract();
    let store: Store = context.extract();
    let Some(tenant) = payments.active_tenant(tenant_id).await? else {
        return Ok(());
    };
    publish_setup(&payments, &tenant, store.0).await
}

/// Writes the tenant's payment setup, as its settings say now, to its
/// streams in `executor`. The settings pages call it too, so the merchant's change
/// shows at once on the storefront instead of when this node's subscription
/// gets to it.
pub async fn publish_setup(
    payments: &Payments,
    tenant: &TenantState,
    executor: Executor,
) -> anyhow::Result<()> {
    let setup = tenant
        .payments
        .as_ref()
        .filter(|_| payments.enabled())
        .map(|settings| PaymentSetup {
            provider: settings.provider.clone(),
            publishable_key: settings.publishable_key.clone(),
        });
    timada_orders::Command::new(executor, &tenant.id)
        .set_payment_method(DASHBOARD_ACTOR, setup)
        .await?;
    Ok(())
}
