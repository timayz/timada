//! Places a shop's paid orders on AliExpress with the company's account: every
//! item of the order imported from AliExpress is bought there, shipped
//! straight to the shopper by AliExpress's own free delivery. The
//! subscription below places the orders the shop wants placed as they come
//! (`auto_forward`) and the ones the merchant asked for; the sweep
//! ([`super::sync`]) reads them again until AliExpress gives their parcel
//! numbers, and fulfils the shop's order with them.

use std::collections::BTreeMap;
use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_aliexpress::aggregator::forward::{Checked, Done, ForwardItem, Requested};
use timada_aliexpress::{
    AeError, AliexpressError, ForwardStatus, LinkState, PlaceItem, PlaceOrder, Placement,
    choose_service, forward_items, link_of_product, logistics_address,
};
use timada_orders::{OrderError, OrderState, OrderStatus};
use timada_tenancy::{CompanyState, TenantState};

use super::{ALIEXPRESS_ACTOR, Aliexpress};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/aliexpress-forward";

/// An order placed longer ago than this is history being replayed: not
/// placed on AliExpress now (the merchant may still ask).
const ORDER_WINDOW_SECS: u64 = 60 * 60;

/// A request older than this is history being replayed.
const REQUEST_WINDOW_SECS: u64 = 10 * 60;

/// How placing one order went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardResult {
    /// AliExpress took it, as these orders.
    Placed(Vec<String>),
    /// Another node took it on, or it is no longer to place.
    Skipped,
    /// Nothing was placed; recorded on the forward.
    Failed(String),
}

/// How reading one placed order again went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// Read; `fulfilled` once the shop's order was fulfilled from it.
    Checked { fulfilled: bool },
    /// No longer read again.
    Done,
    /// Another node read it in between, or it is not placed.
    Skipped,
}

/// Why placing an order failed, for the merchant.
struct Refusal(String);

impl From<AeError> for Refusal {
    fn from(err: AeError) -> Self {
        Refusal(if err.retryable {
            "AliExpress did not answer; try again in a moment.".to_owned()
        } else {
            format!("AliExpress refused: {}", describe(&err))
        })
    }
}

fn describe(err: &AeError) -> String {
    if err.message.is_empty() {
        err.code.clone()
    } else {
        format!("{} ({})", err.message, err.code)
    }
}

impl Aliexpress {
    /// Places the order a forward asks for, on the node that takes it on:
    /// `store` is the store, `read_pool` the read models.
    pub async fn forward_order<E: evento::Executor + Clone>(
        &self,
        store: &E,
        read_pool: &SqlitePool,
        tenant_id: &str,
        forward_id: &str,
    ) -> anyhow::Result<ForwardResult> {
        let forwards = timada_aliexpress::Command::new(store.clone(), tenant_id);
        if !forwards.start_forward(ALIEXPRESS_ACTOR, forward_id).await? {
            return Ok(ForwardResult::Skipped);
        }
        let Some(forward) = forwards.load_forward_by_id(forward_id).await? else {
            return Ok(ForwardResult::Skipped);
        };
        let placed = self
            .place(store, read_pool, tenant_id, &forward.order_id)
            .await;
        match placed {
            Ok(placement) => {
                let ae_order_ids = placement.ae_order_ids.clone();
                forwards
                    .record_placed(ALIEXPRESS_ACTOR, forward_id, placement)
                    .await?;
                tracing::info!(tenant = %tenant_id, order = %forward.order_id, ae_orders = ?ae_order_ids, "order placed on AliExpress");
                Ok(ForwardResult::Placed(ae_order_ids))
            }
            Err(Refusal(reason)) => {
                tracing::warn!(tenant = %tenant_id, order = %forward.order_id, %reason, "order not placed on AliExpress");
                forwards
                    .record_forward_failure(ALIEXPRESS_ACTOR, forward_id, &reason)
                    .await?;
                Ok(ForwardResult::Failed(reason))
            }
        }
    }

    /// Buys an order's AliExpress items for the shopper.
    async fn place<E: evento::Executor + Clone>(
        &self,
        store: &E,
        read_pool: &SqlitePool,
        tenant_id: &str,
        order_id: &str,
    ) -> Result<Placement, Refusal> {
        let failed = |err: anyhow::Error| {
            tracing::error!(%tenant_id, %order_id, error = %err, "AliExpress forward failed");
            Refusal("Placing the order failed on our side; try again.".to_owned())
        };
        let Some(tenant) = self.active_tenant(tenant_id).await.map_err(failed)? else {
            return Err(Refusal("The shop is suspended.".into()));
        };
        let Some(settings) = tenant.aliexpress.as_ref() else {
            return Err(Refusal(
                "Set what the shop buys on AliExpress first.".into(),
            ));
        };
        let Some(company) = self.company_of(&tenant).await.map_err(failed)? else {
            return Err(Refusal("Link the shop to its company first.".into()));
        };
        let Some(client) = self.client_of(&company).map_err(failed)? else {
            return Err(Refusal(
                "Connect the company's AliExpress account first.".into(),
            ));
        };
        let orders = timada_orders::Command::new(store.clone(), tenant_id);
        let Some(order) = orders.load_order(order_id).await.map_err(failed)? else {
            return Err(Refusal("The order does not exist.".into()));
        };
        match order.status {
            OrderStatus::Paid => {}
            OrderStatus::Fulfilled => {
                return Err(Refusal("The order is already fulfilled.".into()));
            }
            OrderStatus::Cancelled => return Err(Refusal("The order is cancelled.".into())),
        }
        let links = order_links(store, read_pool, tenant_id, &order)
            .await
            .map_err(failed)?;
        let plan = forward_items(&order.lines, &links);
        if plan.items.is_empty() {
            return Err(Refusal(
                "No item of this order comes from AliExpress.".into(),
            ));
        }
        let country = order.shipping.country.clone();
        let mut market = Aliexpress::market_of(settings);
        market.ship_to_country = country.clone();
        let mut products = BTreeMap::new();
        let mut items = Vec::new();
        let mut bought = Vec::new();
        for item in &plan.items {
            if !products.contains_key(&item.ae_product_id) {
                let product =
                    client
                        .product(&item.ae_product_id, &market)
                        .await
                        .map_err(|err| {
                            if err.gone {
                                Refusal(format!(
                                    "AliExpress no longer sells product {}.",
                                    item.ae_product_id
                                ))
                            } else {
                                err.into()
                            }
                        })?;
                products.insert(item.ae_product_id.clone(), product);
            }
            let product = &products[&item.ae_product_id];
            if !product.on_sale() {
                return Err(Refusal(format!(
                    "AliExpress no longer sells “{}”.",
                    product.subject
                )));
            }
            let Some(sku) = product.sku(&item.sku_id) else {
                return Err(Refusal(format!(
                    "AliExpress no longer sells this combination of “{}”.",
                    product.subject
                )));
            };
            let attr = match &sku.attr {
                Some(attr) => attr.clone(),
                None if product.skus.len() == 1 => String::new(),
                None => {
                    return Err(Refusal(format!(
                        "AliExpress did not say how to order this combination of “{}”.",
                        product.subject
                    )));
                }
            };
            if sku.available < item.quantity {
                return Err(Refusal(format!(
                    "AliExpress has {} of this combination of “{}” left, the order needs {}.",
                    sku.available, product.subject, item.quantity
                )));
            }
            let options = client
                .freight(
                    &item.ae_product_id,
                    &item.sku_id,
                    item.quantity,
                    &country,
                    &market,
                )
                .await?;
            let Some(service) = choose_service(&options) else {
                return Err(Refusal(format!(
                    "AliExpress offers no free delivery of its own to {country} for “{}”.",
                    product.subject
                )));
            };
            items.push(PlaceItem {
                product_id: item.ae_product_id.clone(),
                sku_attr: attr,
                product_count: item.quantity,
                logistics_service_name: service.service_name.clone(),
                order_memo: order.reference.clone(),
            });
            bought.push(ForwardItem {
                ae_product_id: item.ae_product_id.clone(),
                sku_id: item.sku_id.clone(),
                quantity: item.quantity,
                service_name: service.service_name.clone(),
            });
        }
        let request = PlaceOrder {
            address: logistics_address(&order.shipping, order.language.as_deref()),
            items,
            try_to_pay: settings.try_to_pay,
            pay_currency: settings.currency.clone(),
        };
        let ae_order_ids = client.place_order(&request).await.map_err(|err| {
            if err.may_have_placed() {
                Refusal(
                    "AliExpress did not answer: look at the account's orders on AliExpress before placing this one again."
                        .to_owned(),
                )
            } else {
                Refusal(format!("AliExpress refused the order: {}", describe(&err)))
            }
        })?;
        Ok(Placement {
            ae_order_ids,
            items: bought,
            pay_requested: settings.try_to_pay,
            complete: plan.complete,
        })
    }

    /// Reads a placed order's AliExpress orders again; once each has a
    /// parcel number and every line of the shop's order was bought there,
    /// fulfils the shop's order with them. An order settled otherwise
    /// (fulfilled by hand, cancelled) or watched long enough is no longer
    /// read.
    pub async fn check_forward<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant: &TenantState,
        company: &CompanyState,
        forward_id: &str,
        now: u64,
    ) -> anyhow::Result<CheckResult> {
        let forwards = timada_aliexpress::Command::new(store.clone(), &tenant.id);
        let Some((forward, version)) = forwards.load_forward_versioned(forward_id).await? else {
            return Ok(CheckResult::Skipped);
        };
        if forward.status != ForwardStatus::Placed {
            return Ok(CheckResult::Skipped);
        }
        let Some(client) = self.client_of(company)? else {
            return Ok(CheckResult::Skipped);
        };
        let orders = timada_orders::Command::new(store.clone(), &tenant.id);
        let order = orders.load_order(&forward.order_id).await?;
        let settled = order
            .as_ref()
            .is_none_or(|order| order.status != OrderStatus::Paid);
        let stale = forward
            .placed_at
            .is_some_and(|at| now.saturating_sub(at) > timada_aliexpress::FORWARD_WATCH_SECS);
        let done = |fulfilled: bool, tracking: Option<String>| Done {
            tracking,
            fulfilled,
        };
        let finish = |done: Done| async {
            match forwards
                .record_checked(forward_id, version, Vec::new(), Some(done))
                .await
            {
                Ok(()) => Ok(CheckResult::Done),
                Err(AliexpressError::Write(evento::WriteError::InvalidOriginalVersion)) => {
                    Ok(CheckResult::Skipped)
                }
                Err(err) => Err(anyhow::Error::from(err)),
            }
        };
        if settled || stale {
            return finish(done(false, None)).await;
        }
        let mut checked = Vec::new();
        let mut after = forward.clone();
        for ae_order in &forward.ae_orders {
            let read = match client.order(&ae_order.id).await {
                Ok(read) => read,
                Err(err) => {
                    tracing::warn!(tenant = %tenant.id, ae_order = %ae_order.id, %err, "AliExpress order not read");
                    continue;
                }
            };
            if read.status == ae_order.status
                && read.logistics_status == ae_order.logistics_status
                && read.tracking == ae_order.tracking
            {
                continue;
            }
            if let Some(state) = after.ae_orders.iter_mut().find(|o| o.id == ae_order.id) {
                state.status = read.status.clone();
                state.logistics_status = read.logistics_status.clone();
                state.tracking = read.tracking.clone();
            }
            checked.push(Checked {
                ae_order_id: ae_order.id.clone(),
                status: read.status,
                logistics_status: read.logistics_status,
                tracking: read.tracking,
            });
        }
        let mut finished = None;
        if after.complete && after.all_shipped() {
            let tracking = fit_tracking(&after.tracking());
            match orders
                .fulfil_order(ALIEXPRESS_ACTOR, &forward.order_id, Some(tracking.clone()))
                .await
            {
                Ok(()) => finished = Some(done(true, Some(tracking))),
                Err(OrderError::OrderCancelled | OrderError::OrderNotFound) => {
                    finished = Some(done(false, None))
                }
                Err(err) => return Err(err.into()),
            }
        }
        let fulfilled = finished.as_ref().is_some_and(|done| done.fulfilled);
        match forwards
            .record_checked(forward_id, version, checked, finished)
            .await
        {
            Ok(()) => {
                if fulfilled {
                    tracing::info!(tenant = %tenant.id, order = %forward.order_id, "order fulfilled from AliExpress");
                }
                Ok(CheckResult::Checked { fulfilled })
            }
            Err(AliexpressError::Write(evento::WriteError::InvalidOriginalVersion)) => {
                Ok(CheckResult::Skipped)
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// The parcel numbers, joined, as many as an order's tracking holds.
fn fit_tracking(numbers: &[String]) -> String {
    let mut tracking = String::new();
    for number in numbers {
        let next = if tracking.is_empty() {
            number.clone()
        } else {
            format!("{tracking}, {number}")
        };
        if next.chars().count() > timada_orders::MAX_TRACKING_LEN {
            break;
        }
        tracking = next;
    }
    tracking
}

/// The linked AliExpress items an order's products were imported from.
pub async fn order_links<E: evento::Executor + Clone>(
    store: &E,
    read_pool: &SqlitePool,
    tenant_id: &str,
    order: &OrderState,
) -> anyhow::Result<Vec<LinkState>> {
    let links = timada_aliexpress::Command::new(store.clone(), tenant_id);
    let mut found: Vec<LinkState> = Vec::new();
    for line in &order.lines {
        if found
            .iter()
            .any(|link| link.product_id.as_deref() == Some(line.product_id.as_str()))
        {
            continue;
        }
        let Some(row) = link_of_product(read_pool, tenant_id, &line.product_id).await? else {
            continue;
        };
        if let Some(link) = links.load_link_by_id(&row.id).await?
            && link.is_linked()
        {
            found.push(link);
        }
    }
    Ok(found)
}

/// The store, to write with.
#[derive(Clone)]
struct Store(Evento);

pub fn subscription<E: Executor>(
    aliexpress: Arc<Aliexpress>,
    read_pool: SqlitePool,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(aliexpress)
        .data(read_pool)
        .data(Store(store))
        .handler(on_order_placed())
        .handler(on_forward_requested())
        .any_routing_key()
        .retry(5)
}

/// A paid order of a shop that places its orders on AliExpress as they
/// come: asked for, when it has an item from there.
#[evento::subscription]
async fn on_order_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_orders::aggregator::Placed>,
) -> anyhow::Result<()> {
    if crate::auth::now().saturating_sub(event.timestamp) > ORDER_WINDOW_SECS {
        return Ok(());
    }
    let aliexpress: Arc<Aliexpress> = context.extract();
    let read_pool: SqlitePool = context.extract();
    let store: Store = context.extract();
    let Some(merchant) = aliexpress.active_tenant(&event.tenant).await? else {
        return Ok(());
    };
    let wanted = merchant
        .aliexpress
        .as_ref()
        .is_some_and(|settings| settings.auto_forward)
        && aliexpress.connected(&merchant).await?;
    if !wanted {
        return Ok(());
    }
    let orders = timada_orders::Command::new(store.0.clone(), &event.tenant);
    let Some(order) = orders.load_order(&event.aggregate_id).await? else {
        return Ok(());
    };
    let links = order_links(&store.0, &read_pool, &event.tenant, &order).await?;
    if forward_items(&order.lines, &links).items.is_empty() {
        return Ok(());
    }
    match timada_aliexpress::Command::new(store.0.clone(), &event.tenant)
        .request_forward(ALIEXPRESS_ACTOR, &order.id, false)
        .await
    {
        Ok(_) | Err(AliexpressError::AlreadyForwarded | AliexpressError::Forwarding) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[evento::subscription]
async fn on_forward_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Requested>,
) -> anyhow::Result<()> {
    if crate::auth::now().saturating_sub(event.timestamp) > REQUEST_WINDOW_SECS {
        return Ok(());
    }
    let aliexpress: Arc<Aliexpress> = context.extract();
    let read_pool: SqlitePool = context.extract();
    let store: Store = context.extract();
    aliexpress
        .forward_order(&store.0, &read_pool, &event.tenant, &event.aggregate_id)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_keeps_what_fits() {
        assert_eq!(fit_tracking(&["A1".into(), "B2".into()]), "A1, B2");
        let long = "X".repeat(150);
        assert_eq!(fit_tracking(&[long.clone(), long.clone()]), long);
    }
}
