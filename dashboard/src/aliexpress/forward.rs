//! Places a shop's paid orders on AliExpress with the company's account: every
//! item of the order imported from AliExpress is bought there, shipped
//! straight to the shopper by AliExpress's own delivery. The
//! subscription below places the orders the shop wants placed as they come
//! (`auto_forward`) and the ones the merchant asked for. Each push from
//! AliExpress ([`super::webhook`]) reads them again, the sweep ([`super::sync`])
//! when no push came for a while, until AliExpress gives their parcel numbers,
//! and the shop's order is fulfilled with them.
//!
//! An order is placed on AliExpress, on the company's account, for a
//! payment the provider holds: one a `store:write` key forged in the shop's
//! store names no payment the provider knows as the shop's, and is refused
//! (timada #431).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_aliexpress::aggregator::forward::{Checked, Done, ForwardItem, Requested};
use timada_aliexpress::{
    ALIEXPRESS_SUPPLIER, AeError, AliexpressError, Client, ForwardStatus, LinkState, PlaceItem,
    PlaceOrder, PlacedRecord, Placement, choose_service, delivery_country, describe_options,
    forward_items, logistics_address,
};
use timada_orders::{OrderError, OrderLine, OrderStatus};
use timada_tenancy::{CompanyState, TenantState};

use super::{ALIEXPRESS_ACTOR, Aliexpress};
use crate::payments::{OrderPayment, Payments};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/aliexpress-forward";

/// An order placed longer ago than this is history being replayed: not
/// placed on AliExpress now (the merchant may still ask).
const ORDER_WINDOW_SECS: u64 = 60 * 60;

/// A request older than this is history being replayed.
const REQUEST_WINDOW_SECS: u64 = 10 * 60;

/// How many times what AliExpress took is written before it is only logged,
/// and the pause before the second try, doubled each time after: writes
/// paused for a moment, a node rejoining, another write at the same moment.
const RECORD_ATTEMPTS: u32 = 5;
#[cfg(not(test))]
const RECORD_RETRY_PAUSE: Duration = Duration::from_millis(500);
#[cfg(test)]
const RECORD_RETRY_PAUSE: Duration = Duration::from_millis(5);

/// Why a forward failed when AliExpress may have the order all the same: it
/// did not answer the order, or the node placing it stopped, or could not
/// record the answer. Never placed again on its own.
pub const MAY_HAVE_PLACED: &str = "AliExpress did not answer: look at the account's orders on AliExpress before placing this one again.";

/// How placing one order went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardResult {
    /// AliExpress took it, as these orders.
    Placed(Vec<String>),
    /// AliExpress took it a second time, as these orders, for a forward an
    /// overlapping attempt placed: recorded beside them, to cancel there.
    AlsoPlaced(Vec<String>),
    /// AliExpress took it, as these orders, and the store would not record
    /// them: kept on this node (and in the log) until the sweep records them.
    Unrecorded(Vec<String>),
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
            tracing::warn!(reason = %describe(&err), "AliExpress call to retry");
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
    /// `store` is the store, `db` this node's own record of what AliExpress
    /// took and the store would not take yet, `payments` says whether the
    /// provider holds the order's money.
    pub async fn forward_order<E: evento::Executor + Clone>(
        &self,
        store: &E,
        db: &Db,
        payments: &Payments,
        tenant_id: &str,
        forward_id: &str,
    ) -> anyhow::Result<ForwardResult> {
        let forwards = timada_aliexpress::Command::new(store.clone(), tenant_id);
        if !forwards.start_forward(ALIEXPRESS_ACTOR, forward_id).await? {
            return Ok(ForwardResult::Skipped);
        }
        let Some((forward, version)) = forwards.load_forward_versioned(forward_id).await? else {
            return Ok(ForwardResult::Skipped);
        };
        let placed = match self
            .prepare(store, payments, tenant_id, &forward.order_id)
            .await
        {
            Ok(prepared) => {
                // The forward's fence, at the version this attempt took it
                // on at: one overtaken meanwhile (asked for again, given up
                // as stalled) asks AliExpress nothing.
                if !forwards
                    .begin_asking(ALIEXPRESS_ACTOR, forward_id, version)
                    .await?
                {
                    tracing::warn!(tenant = %tenant_id, order = %forward.order_id, forward = %forward_id, "placing an order on AliExpress was overtaken: this attempt asks nothing");
                    return Ok(ForwardResult::Skipped);
                }
                self.ask(store, tenant_id, &forward.order_id, prepared)
                    .await?
            }
            Err(refusal) => Err(refusal),
        };
        match placed {
            Ok(placement) => {
                let ae_order_ids = placement.ae_order_ids.clone();
                match record_placement(&forwards, forward_id, placement.clone()).await {
                    Ok(PlacedRecord::Placed) => {
                        tracing::info!(tenant = %tenant_id, order = %forward.order_id, ae_orders = ?ae_order_ids, "order placed on AliExpress");
                        Ok(ForwardResult::Placed(ae_order_ids))
                    }
                    Ok(PlacedRecord::AlsoPlaced) => {
                        tracing::error!(tenant = %tenant_id, order = %forward.order_id, forward = %forward_id, ae_orders = ?ae_order_ids, "AliExpress took an order already placed there a second time: recorded on the forward, to cancel there");
                        Ok(ForwardResult::AlsoPlaced(ae_order_ids))
                    }
                    Err(err) if forward_holds(&forwards, forward_id, &ae_order_ids).await => {
                        // On the forward: only the platform's record of the
                        // numbers failed, which pushes find it by. The sweep
                        // reads the order meanwhile.
                        if matches!(err, AliexpressError::ClaimedElsewhere { .. }) {
                            tracing::error!(tenant = %tenant_id, order = %forward.order_id, forward = %forward_id, ae_orders = ?ae_order_ids, error = %err, "an AliExpress order of this forward is recorded for another one");
                        } else {
                            tracing::warn!(tenant = %tenant_id, order = %forward.order_id, forward = %forward_id, ae_orders = ?ae_order_ids, error = %err, "AliExpress orders recorded on the forward but not for its pushes: read by the sweep");
                        }
                        Ok(ForwardResult::Placed(ae_order_ids))
                    }
                    Err(err) => {
                        // AliExpress has the order and the store would not
                        // take its record: kept on this node until the sweep
                        // records it, and in the log.
                        tracing::error!(tenant = %tenant_id, order = %forward.order_id, forward = %forward_id, ae_orders = ?ae_order_ids, error = %err, "order placed on AliExpress but not recorded yet: kept on this node for the sweep");
                        if let Err(kept) = timada_aliexpress::keep_unrecorded(
                            db,
                            tenant_id,
                            forward_id,
                            &forward.order_id,
                            &placement,
                            crate::auth::now(),
                            &err.to_string(),
                        )
                        .await
                        {
                            tracing::error!(tenant = %tenant_id, order = %forward.order_id, forward = %forward_id, ae_orders = ?ae_order_ids, error = %kept, "nor kept on this node: look for these AliExpress orders before placing it again");
                        }
                        Ok(ForwardResult::Unrecorded(ae_order_ids))
                    }
                }
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

    /// Reads all that buying an order's AliExpress items for the shopper
    /// needs, and what to ask AliExpress for; or why it cannot be bought.
    async fn prepare<E: evento::Executor + Clone>(
        &self,
        store: &E,
        payments: &Payments,
        tenant_id: &str,
        order_id: &str,
    ) -> Result<Prepared, Refusal> {
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
        // Checked here to say why early; the order's fence checks again, at
        // the very moment AliExpress is asked.
        order
            .may_order_from_supplier(crate::auth::now())
            .map_err(|err| not_placeable(err).unwrap_or_else(|err| failed(err.into())))?;
        // The provider's word, not the order's: the money is held for what
        // the company is about to buy.
        let held = payments.order_payment(&tenant, &order).await.map_err(|err| {
            tracing::warn!(%tenant_id, %order_id, error = %err, "the payment provider did not answer about an order to place on AliExpress");
            Refusal("The payment provider did not answer; try again in a moment.".to_owned())
        })?;
        match held {
            OrderPayment::Held => {}
            OrderPayment::Gone => {
                return Err(Refusal(
                    "The provider no longer holds the order's payment.".into(),
                ));
            }
            OrderPayment::Unknown => {
                tracing::warn!(%tenant_id, %order_id, payment = %order.payment.payment_id, "an order to place on AliExpress names a payment the provider does not know as this shop's");
                return Err(Refusal(
                    "The provider holds no payment for this order: it was not placed through the shop's checkout.".into(),
                ));
            }
        }
        let links = order_links(store, tenant_id, &order.lines)
            .await
            .map_err(failed)?;
        let plan = forward_items(&order.lines, &links);
        if plan.items.is_empty() {
            return Err(Refusal(
                "No item of this order comes from AliExpress.".into(),
            ));
        }
        let country = delivery_country(&order.shipping.country, &order.shipping.postal_code);
        let mut market = Aliexpress::market_of(settings);
        market.ship_to_country = country.clone();
        let mut address = logistics_address(
            &order.shipping,
            order.language.as_deref(),
            settings.order_phone.as_deref(),
        );
        let places = timada_aliexpress::places_of(&country);
        if let Some(places) = places {
            places.apply(&mut address);
        }
        let province = places
            .filter(|places| places.names_province(&address.province))
            .map(|_| address.province.clone());
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
                    province.as_deref(),
                    &market,
                )
                .await?;
            let Some(service) = choose_service(&options) else {
                return Err(Refusal(if options.is_empty() {
                    format!(
                        "AliExpress offers no delivery to {country} for “{}”.",
                        product.subject
                    )
                } else {
                    format!(
                        "No delivery of “{}” to {country} on AliExpress can be followed; offered: {}.",
                        product.subject,
                        describe_options(&options, &market.currency)
                    )
                }));
            };
            tracing::info!(
                tenant = %tenant_id,
                order = %order_id,
                ae_product_id = %item.ae_product_id,
                service = %service.service_name,
                fee_cents = service.cents,
                "AliExpress delivery chosen"
            );
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
        if address.mobile_no.is_empty() {
            return Err(Refusal(
                "AliExpress needs a phone number: the shopper gave none and the shop has none set for its orders on the AliExpress page."
                    .to_owned(),
            ));
        }
        Ok(Prepared {
            client,
            request: PlaceOrder {
                address,
                items,
                try_to_pay: settings.try_to_pay,
                pay_currency: settings.currency.clone(),
            },
            bought,
            complete: plan.complete,
        })
    }

    /// Asks AliExpress for a prepared order: what it took, or why nothing
    /// was placed. An error is a store that would not record that AliExpress
    /// may have the order: the forward is left taken on, for the sweep.
    async fn ask<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant_id: &str,
        order_id: &str,
        prepared: Prepared,
    ) -> anyhow::Result<Result<Placement, Refusal>> {
        let orders = timada_orders::Command::new(store.clone(), tenant_id);
        // The fence: written on the order at the version read, right before
        // AliExpress is asked, so a shopper or merchant cancelling at the same
        // moment either stops it here or finds it written.
        if let Err(err) = orders
            .begin_supplier_order(
                ALIEXPRESS_ACTOR,
                order_id,
                ALIEXPRESS_SUPPLIER,
                crate::auth::now(),
            )
            .await
        {
            return Ok(Err(not_placeable(err).unwrap_or_else(|err| {
                tracing::error!(%tenant_id, %order_id, error = %err, "AliExpress forward failed");
                Refusal("Placing the order failed on our side; try again.".to_owned())
            })));
        }
        let ae_order_ids = match prepared.client.place_order(&prepared.request).await {
            Ok(ids) => ids,
            // Either way the order is told before the forward is, tried like
            // a placement's record: an attempt left open on a failed forward
            // would keep the fence for good. One the store never takes leaves
            // the forward taken on, for the stall sweep, which tells the order
            // AliExpress may have it.
            Err(err) if err.may_have_placed() => {
                // A later attempt that AliExpress refuses must not take the
                // fence down while this one may be there.
                order_write(order_id, || {
                    orders.record_supplier_uncertainty(
                        ALIEXPRESS_ACTOR,
                        order_id,
                        ALIEXPRESS_SUPPLIER,
                        MAY_HAVE_PLACED,
                    )
                })
                .await?;
                return Ok(Err(Refusal(MAY_HAVE_PLACED.to_owned())));
            }
            Err(err) => {
                let reason = format!("AliExpress refused the order: {}", describe(&err));
                // It surely took nothing: the shopper may cancel again, unless
                // an earlier attempt may be there.
                order_write(order_id, || {
                    orders.record_supplier_refusal(
                        ALIEXPRESS_ACTOR,
                        order_id,
                        ALIEXPRESS_SUPPLIER,
                        &reason,
                    )
                })
                .await?;
                return Ok(Err(Refusal(reason)));
            }
        };
        Ok(Ok(Placement {
            ae_order_ids,
            items: prepared.bought,
            pay_requested: prepared.request.try_to_pay,
            complete: prepared.complete,
        }))
    }

    /// Fulfils a paid order from the parcels its forward already knows,
    /// when every line was bought on AliExpress and every one shipped:
    /// whether it did. For an order kept open without its hold, whose
    /// forward may have closed with every parcel known and nothing to
    /// fulfil, once the merchant says the money came in outside timada.
    /// An order not paid, left, or cancelled, or a forward with parcels
    /// still to come (the next read fulfils it), changes nothing.
    pub async fn fulfil_settled_order<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant_id: &str,
        order_id: &str,
    ) -> anyhow::Result<bool> {
        let orders = timada_orders::Command::new(store.clone(), tenant_id);
        // Paid and not left: anything else has nothing to fulfil here.
        if !orders
            .load_order(order_id)
            .await?
            .is_some_and(|order| order.status == OrderStatus::Paid)
        {
            return Ok(false);
        }
        let Some(forward) = timada_aliexpress::Command::new(store.clone(), tenant_id)
            .load_forward(order_id)
            .await?
        else {
            return Ok(false);
        };
        if !matches!(forward.status, ForwardStatus::Placed | ForwardStatus::Done)
            || !forward.complete
            || !forward.all_shipped()
        {
            return Ok(false);
        }
        let tracking = fit_tracking(&forward.tracking());
        match orders
            .fulfil_order(ALIEXPRESS_ACTOR, order_id, Some(tracking))
            .await
        {
            Ok(()) => {
                tracing::info!(tenant = %tenant_id, order = %order_id, "order settled outside timada fulfilled from AliExpress");
                Ok(true)
            }
            Err(
                OrderError::OrderCancelled
                | OrderError::OrderNotFound
                | OrderError::PaymentNotCaptured
                | OrderError::CancellationPending,
            ) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Reads a placed order's AliExpress orders again; once each has a
    /// parcel number and every line of the shop's order was bought there,
    /// fulfils the shop's order with them. Once AliExpress closed every one
    /// of them shipping nothing (`closed` names the one its push said it
    /// closed), the shop's order is to be cancelled: the payments sweep
    /// gives the hold back, or the money. An order settled otherwise
    /// (fulfilled by hand, cancelled), watched long enough, or kept open
    /// without its hold once every parcel is known is no longer read; the
    /// merchant saying the money came in outside timada fulfils that last
    /// one from the parcels known ([`fulfil_settled_order`](Self::fulfil_settled_order)).
    ///
    /// Written in three steps, as the shop's order is another stream: what
    /// was read, the shop's order, then `Done`. A node stopping in between
    /// leaves the next read what it needs, whether AliExpress answers or
    /// not: the reads to fulfil from, or an order fulfilled from here whose
    /// forward only has to be closed.
    pub async fn check_forward<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant: &TenantState,
        company: &CompanyState,
        forward_id: &str,
        closed: Option<&str>,
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
        let settled = order.as_ref().is_none_or(|order| order.is_settled());
        // An order whose money is not taken yet is not fulfilled from here:
        // its parcels wait in the reads until it is.
        let paid = order
            .as_ref()
            .is_some_and(|order| order.status == OrderStatus::Paid);
        let stale = forward
            .placed_at
            .is_some_and(|at| now.saturating_sub(at) > timada_aliexpress::FORWARD_WATCH_SECS);
        // Every parcel known of an order whose hold went back with nothing
        // taken (kept open for the merchant): it can neither be fulfilled
        // from here nor closed by AliExpress any more, so nothing a read
        // finds changes anything.
        let unpayable = order
            .as_ref()
            .is_some_and(|order| order.released_at.is_some() && !order.is_paid());
        let shipped_unpayable = forward.complete && forward.all_shipped() && unpayable;
        let done = |fulfilled: bool, tracking: Option<String>| Done {
            tracking,
            fulfilled,
        };
        let finish = |done: Done| async {
            match forwards
                .record_checked(forward_id, version, Vec::new(), Some(done))
                .await
            {
                Ok(_) => Ok(CheckResult::Done),
                Err(AliexpressError::Write(evento::WriteError::InvalidOriginalVersion)) => {
                    Ok(CheckResult::Skipped)
                }
                Err(err) => Err(anyhow::Error::from(err)),
            }
        };
        // Fulfilled by a read that stopped before closing the forward, or by
        // another node's that is about to: closed as that read would have.
        if let Some(order) =
            order.filter(|order| order.fulfilled_by.as_deref() == Some(ALIEXPRESS_ACTOR))
        {
            tracing::info!(tenant = %tenant.id, order = %forward.order_id, "order already fulfilled from AliExpress, closing its forward");
            return finish(done(true, order.tracking)).await;
        }
        if settled || stale || shipped_unpayable {
            return finish(done(false, None)).await;
        }
        let mut checked = Vec::new();
        let mut after = forward.clone();
        for ae_order in &forward.ae_orders {
            let pushed_closed = closed == Some(ae_order.id.as_str());
            let read = match client.order(&ae_order.id).await {
                Ok(read) => read,
                // The push is signed: what it says stands while the order
                // cannot be read.
                Err(err) if pushed_closed && !ae_order.closed => {
                    tracing::warn!(tenant = %tenant.id, ae_order = %ae_order.id, %err, "AliExpress order not read: closed as its push says");
                    timada_aliexpress::AeOrder {
                        status: ae_order.status.clone(),
                        logistics_status: ae_order.logistics_status.clone(),
                        tracking: ae_order.tracking.clone(),
                    }
                }
                Err(err) => {
                    tracing::warn!(tenant = %tenant.id, ae_order = %ae_order.id, %err, "AliExpress order not read");
                    continue;
                }
            };
            // Closed shipping nothing: its push said so, or it finished
            // with no parcel ever known.
            let closed = ae_order.closed
                || pushed_closed
                || (read.status == AE_FINISHED
                    && read.tracking.is_empty()
                    && ae_order.tracking.is_empty());
            if read.status == ae_order.status
                && read.logistics_status == ae_order.logistics_status
                && read.tracking == ae_order.tracking
                && closed == ae_order.closed
            {
                continue;
            }
            if let Some(state) = after.ae_orders.iter_mut().find(|o| o.id == ae_order.id) {
                state.status = read.status.clone();
                state.logistics_status = read.logistics_status.clone();
                state.tracking = read.tracking.clone();
                state.closed = closed;
            }
            checked.push(Checked {
                ae_order_id: ae_order.id.clone(),
                status: read.status,
                logistics_status: read.logistics_status,
                tracking: read.tracking,
                closed,
            });
        }
        let version = match forwards
            .record_checked(forward_id, version, checked, None)
            .await
        {
            Ok(version) => version,
            Err(AliexpressError::Write(evento::WriteError::InvalidOriginalVersion)) => {
                return Ok(CheckResult::Skipped);
            }
            Err(err) => return Err(err.into()),
        };
        // Nothing will ship of an order AliExpress alone was to fill: it is
        // cancelled, its money given back by the payments sweep. A mixed
        // order, or one only partly closed, is the merchant's to settle.
        if after.complete && after.all_closed() {
            match orders
                .request_cancellation(
                    ALIEXPRESS_ACTOR,
                    &forward.order_id,
                    CLOSED_BY_ALIEXPRESS.to_owned(),
                    true,
                )
                .await
            {
                Ok(_) => {
                    tracing::info!(tenant = %tenant.id, order = %forward.order_id, "AliExpress closed the order: the shop's order is cancelled");
                }
                // Its money is being taken, or it left: read again later.
                Err(OrderError::CapturePending | OrderError::NotCancellable) => {}
                Err(err) => return Err(err.into()),
            }
            return Ok(CheckResult::Checked { fulfilled: false });
        }
        if !after.complete || !after.all_shipped() || !paid {
            return Ok(CheckResult::Checked { fulfilled: false });
        }
        let tracking = fit_tracking(&after.tracking());
        let finished = match orders
            .fulfil_order(ALIEXPRESS_ACTOR, &forward.order_id, Some(tracking.clone()))
            .await
        {
            Ok(()) => done(true, Some(tracking)),
            Err(OrderError::OrderCancelled | OrderError::OrderNotFound) => done(false, None),
            // Not taken, or being cancelled, after all: read again later.
            Err(OrderError::PaymentNotCaptured | OrderError::CancellationPending) => {
                return Ok(CheckResult::Checked { fulfilled: false });
            }
            Err(err) => return Err(err.into()),
        };
        let fulfilled = finished.fulfilled;
        match forwards
            .record_checked(forward_id, version, Vec::new(), Some(finished))
            .await
        {
            Ok(_) => {
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

/// The status AliExpress gives an order it is done with: delivered, or
/// closed.
const AE_FINISHED: &str = "FINISH";

/// Why a shop's order is cancelled when AliExpress closed every order of it.
pub const CLOSED_BY_ALIEXPRESS: &str = "AliExpress closed the order without shipping it.";

/// Why an order is not placed on AliExpress, for the merchant; the error
/// back when it is no refusal of the order (the store failing).
fn not_placeable(err: OrderError) -> Result<Refusal, OrderError> {
    Ok(Refusal(
        match err {
            OrderError::OrderCancelled => "The order is cancelled.",
            OrderError::OrderFulfilled => "The order is already fulfilled.",
            OrderError::CancellationPending => "The order is being cancelled.",
            OrderError::HoldExpiring => {
                "The hold on the shopper's card ends too soon to buy the goods: take the payment first, then place it again."
            }
            OrderError::OrderNotFound => "The order does not exist.",
            err => return Err(err),
        }
        .to_owned(),
    ))
}

/// Writes to the shop's order with `write`, trying again while the store
/// does not take it, as a placement's record is; the last error once every
/// try failed. A refusal of the order itself is not tried again.
async fn order_write<F, Fut>(order_id: &str, mut write: F) -> Result<(), OrderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), OrderError>>,
{
    let mut pause = RECORD_RETRY_PAUSE;
    let mut attempt = 1;
    loop {
        match write().await {
            Ok(()) => return Ok(()),
            Err(err @ (OrderError::Write(_) | OrderError::Other(_)))
                if attempt < RECORD_ATTEMPTS =>
            {
                tracing::warn!(order = %order_id, attempt, error = %err, "the order is not told how placing it went yet: trying again");
                tokio::time::sleep(pause).await;
                pause *= 2;
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// What placing asks AliExpress for, once all it needs was read.
struct Prepared {
    client: Client,
    request: PlaceOrder,
    bought: Vec<ForwardItem>,
    /// Every line of the shop's order is bought there.
    complete: bool,
}

/// Whether the forward holds every one of `ae_order_ids`, placed or beside
/// what was; `false` when it cannot be read.
async fn forward_holds<E: evento::Executor>(
    forwards: &timada_aliexpress::Command<E>,
    forward_id: &str,
    ae_order_ids: &[String],
) -> bool {
    let Ok(Some(forward)) = forwards.load_forward_by_id(forward_id).await else {
        return false;
    };
    ae_order_ids.iter().all(|id| {
        forward.ae_orders.iter().any(|order| &order.id == id) || forward.also_placed.contains(id)
    })
}

/// Records the orders AliExpress took, trying again while the store does not
/// take the write (each try reads the forward again); the last error once
/// every try failed. A refusal of the forward itself is not tried again.
async fn record_placement<E: evento::Executor>(
    forwards: &timada_aliexpress::Command<E>,
    forward_id: &str,
    placement: Placement,
) -> Result<PlacedRecord, AliexpressError> {
    let mut pause = RECORD_RETRY_PAUSE;
    let mut attempt = 1;
    loop {
        match forwards
            .record_placed(ALIEXPRESS_ACTOR, forward_id, placement.clone())
            .await
        {
            Ok(record) => return Ok(record),
            Err(err @ (AliexpressError::Write(_) | AliexpressError::Other(_)))
                if attempt < RECORD_ATTEMPTS =>
            {
                tracing::warn!(forward = %forward_id, attempt, error = %err, "orders AliExpress took not recorded yet: trying again");
                tokio::time::sleep(pause).await;
                pause *= 2;
                attempt += 1;
            }
            Err(err) => return Err(err),
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

/// The linked AliExpress items the products of an order's lines were
/// imported from, read from the store (the product's index stream, then the
/// item's): what is bought, and whether a payment waits for the purchase,
/// is decided here, so a read model a moment behind never buys the wrong
/// thing or takes the money early.
pub async fn order_links<E: evento::Executor + Clone>(
    store: &E,
    tenant_id: &str,
    lines: &[OrderLine],
) -> anyhow::Result<Vec<LinkState>> {
    let links = timada_aliexpress::Command::new(store.clone(), tenant_id);
    let mut found: Vec<LinkState> = Vec::new();
    for line in lines {
        if found
            .iter()
            .any(|link| link.product_id.as_deref() == Some(line.product_id.as_str()))
        {
            continue;
        }
        if let Some(link) = links.load_link_of_product(&line.product_id).await? {
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
    db: Db,
    payments: Arc<Payments>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(aliexpress)
        .data(db)
        .data(payments)
        .data(Store(store))
        .handler(on_order_placed())
        .handler(on_forward_requested())
        .handler(on_settled_outside())
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
    let links = order_links(&store.0, &event.tenant, &order.lines).await?;
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

/// An order kept open whose money the merchant collected outside timada is
/// paid: fulfilled from the parcels AliExpress already shipped, when every
/// one is known. Otherwise the next read fulfils it, as any paid order.
#[evento::subscription]
async fn on_settled_outside<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_orders::aggregator::SettledOutside>,
) -> anyhow::Result<()> {
    let aliexpress: Arc<Aliexpress> = context.extract();
    let store: Store = context.extract();
    aliexpress
        .fulfil_settled_order(&store.0, &event.tenant, &event.aggregate_id)
        .await?;
    Ok(())
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
    let db: Db = context.extract();
    let payments: Arc<Payments> = context.extract();
    let store: Store = context.extract();
    aliexpress
        .forward_order(&store.0, &db, &payments, &event.tenant, &event.aggregate_id)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use evento::cursor::{Args, ReadResult, Value};
    use evento::{EventFilter, RoutingKey, SubscriberStatus, TenantFilter, WriteError};
    use timada_aliexpress::{ALIEXPRESS_SUPPLIER, ForwardState, Placement};
    use timada_orders::{OrderState, Payment, ShippingAddress};
    use timada_tenancy::AliexpressAuthorization;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use ulid::Ulid;

    use super::*;
    use crate::config::AliexpressConfig;
    use crate::seal::Sealer;

    const TENANT: &str = "tenant-1";
    const ORDER: &str = "order-1";

    #[test]
    fn tracking_keeps_what_fits() {
        assert_eq!(fit_tracking(&["A1".into(), "B2".into()]), "A1, B2");
        let long = "X".repeat(150);
        assert_eq!(fit_tracking(&[long.clone(), long.clone()]), long);
    }

    /// The store of a node that stops the moment it would write `event`: what
    /// it wrote before is there, that write and anything after it is not.
    #[derive(Clone)]
    struct StopsAt {
        store: evento::Turso,
        event: &'static str,
    }

    #[async_trait::async_trait]
    impl Executor for StopsAt {
        async fn write(&self, events: Vec<evento::Event>) -> Result<(), WriteError> {
            if events.iter().any(|event| event.name == self.event) {
                return Err(WriteError::Unknown(anyhow::anyhow!(
                    "stopped at {}",
                    self.event
                )));
            }
            self.store.write(events).await
        }

        async fn read(
            &self,
            aggregators: Option<Arc<[EventFilter]>>,
            routing_key: Option<RoutingKey>,
            tenant: TenantFilter,
            args: Args,
            to_micros: Option<u64>,
        ) -> anyhow::Result<ReadResult<evento::Event>> {
            self.store
                .read(aggregators, routing_key, tenant, args, to_micros)
                .await
        }

        async fn latest_timestamp(
            &self,
            aggregators: Option<Arc<[EventFilter]>>,
            routing_key: Option<RoutingKey>,
            tenant: TenantFilter,
        ) -> anyhow::Result<u64> {
            self.store
                .latest_timestamp(aggregators, routing_key, tenant)
                .await
        }

        async fn get_subscriber_cursor(&self, key: String) -> anyhow::Result<Option<Value>> {
            self.store.get_subscriber_cursor(key).await
        }

        async fn is_subscriber_running(
            &self,
            key: String,
            worker_id: Ulid,
        ) -> anyhow::Result<bool> {
            self.store.is_subscriber_running(key, worker_id).await
        }

        async fn subscriber_status(
            &self,
            key: String,
            worker_id: Ulid,
        ) -> anyhow::Result<SubscriberStatus> {
            self.store.subscriber_status(key, worker_id).await
        }

        async fn latest_version(
            &self,
            tenant: String,
            aggregate_type: String,
            aggregate_id: String,
        ) -> anyhow::Result<u16> {
            self.store
                .latest_version(tenant, aggregate_type, aggregate_id)
                .await
        }

        async fn stream_routing_key(
            &self,
            tenant: String,
            aggregate_type: String,
            aggregate_id: String,
        ) -> anyhow::Result<Option<Option<String>>> {
            self.store
                .stream_routing_key(tenant, aggregate_type, aggregate_id)
                .await
        }

        async fn upsert_subscriber(&self, key: String, worker_id: Ulid) -> anyhow::Result<()> {
            self.store.upsert_subscriber(key, worker_id).await
        }

        async fn acknowledge(
            &self,
            key: String,
            worker_id: Ulid,
            cursor: Value,
            lag: u64,
        ) -> anyhow::Result<bool> {
            self.store.acknowledge(key, worker_id, cursor, lag).await
        }

        async fn get_snapshot(
            &self,
            tenant: String,
            aggregate_type: String,
            projection: String,
            aggregate_revision: String,
            id: String,
        ) -> anyhow::Result<Option<(Vec<u8>, Value)>> {
            self.store
                .get_snapshot(tenant, aggregate_type, projection, aggregate_revision, id)
                .await
        }

        async fn save_snapshot(
            &self,
            tenant: String,
            aggregate_type: String,
            projection: String,
            aggregate_revision: String,
            id: String,
            data: Vec<u8>,
            cursor: Value,
        ) -> anyhow::Result<()> {
            self.store
                .save_snapshot(
                    tenant,
                    aggregate_type,
                    projection,
                    aggregate_revision,
                    id,
                    data,
                    cursor,
                )
                .await
        }

        async fn delete_snapshot(
            &self,
            tenant: String,
            aggregate_type: String,
            projection: String,
            id: String,
        ) -> anyhow::Result<()> {
            self.store
                .delete_snapshot(tenant, aggregate_type, projection, id)
                .await
        }
    }

    /// A stand-in for AliExpress: answers `aliexpress.trade.ds.order.get`.
    #[derive(Default)]
    struct StandIn {
        /// The parcel number of each order that shipped.
        shipped: Mutex<HashMap<String, String>>,
        /// The orders AliExpress closed without shipping them.
        closed: Mutex<Vec<String>>,
        /// Answers 503 to everything.
        down: AtomicBool,
        reads: AtomicUsize,
    }

    impl StandIn {
        /// Listens on a port of its own; its address.
        async fn start(self: Arc<Self>) -> String {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let answer = self.answer(&request_of(&mut socket).await);
                    socket.write_all(answer.as_bytes()).await.unwrap();
                }
            });
            url
        }

        fn answer(&self, request: &str) -> String {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.down.load(Ordering::SeqCst) {
                return "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned();
            }
            let path = request.split(' ').nth(1).unwrap_or_default();
            let url = reqwest::Url::parse(&format!("http://stand-in{path}")).unwrap();
            let id = url
                .query_pairs()
                .find(|(name, _)| name == "order_id")
                .map(|(_, id)| id.into_owned())
                .unwrap_or_default();
            let closed = self.closed.lock().unwrap().contains(&id);
            let result = match self.shipped.lock().unwrap().get(&id) {
                None if closed => serde_json::json!({
                    "order_status": "FINISH",
                    "logistics_status": "NO_LOGISTICS",
                }),
                Some(parcel) => serde_json::json!({
                    "order_status": "WAIT_BUYER_ACCEPT_GOODS",
                    "logistics_status": "SELLER_SEND_GOODS",
                    "logistics_info_list": {
                        "ae_order_logistics_info": [{
                            "logistics_no": parcel,
                            "logistics_service": "CAINIAO_STANDARD",
                        }],
                    },
                }),
                None => serde_json::json!({
                    "order_status": "WAIT_SELLER_SEND_GOODS",
                    "logistics_status": "NO_LOGISTICS",
                }),
            };
            let body = serde_json::json!({
                "aliexpress_trade_ds_order_get_response": { "rsp_code": "200", "result": result },
            })
            .to_string();
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        }
    }

    async fn request_of(socket: &mut tokio::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        while !request.windows(4).any(|end| end == b"\r\n\r\n") {
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => request.extend_from_slice(&chunk[..read]),
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    /// A shop connected to AliExpress, on a store of its own.
    struct Shop {
        store: evento::Turso,
        aliexpress: Arc<Aliexpress>,
        stand_in: Arc<StandIn>,
        tenant: TenantState,
        company: CompanyState,
    }

    impl Shop {
        async fn new() -> Self {
            let store = evento::Turso::in_memory_concurrent().await.unwrap();
            let stand_in = Arc::new(StandIn::default());
            let config = AliexpressConfig {
                app_key: Some("key".to_owned()),
                app_secret: Some("secret".to_owned()),
                api_url: Some(stand_in.clone().start().await),
                authorize_url: None,
                sync_check_secs: 1,
                sync_every_secs: 1,
                order_read_every_secs: 86_400,
                calls_per_day: 86_400,
                read_fresh_secs: 60,
            };
            let aliexpress = Aliexpress::new(
                Some(Sealer::new("aliexpress-test", "secret")),
                &config,
                false,
                Evento::new(store.clone()),
                Arc::new(|| timada_aliexpress::Share::ALONE),
            )
            .unwrap();
            let company = CompanyState {
                id: "company-1".to_owned(),
                aliexpress: Some(AliexpressAuthorization {
                    access_token_sealed: aliexpress.seal("company-1", "token").unwrap(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let tenant = TenantState {
                id: TENANT.to_owned(),
                ..Default::default()
            };
            Self {
                store,
                aliexpress,
                stand_in,
                tenant,
                company,
            }
        }

        /// A paid order placed on AliExpress as `ae_order_ids`; its forward.
        async fn placed(&self, ae_order_ids: &[&str]) -> String {
            self.placed_with(ae_order_ids, true).await
        }

        /// An order placed on AliExpress as `ae_order_ids`, its money taken
        /// (`paid`) or still held; its forward.
        async fn placed_with(&self, ae_order_ids: &[&str], paid: bool) -> String {
            let mut write = evento::append(ORDER);
            write
                .tenant(TENANT)
                .original_version(0)
                .event(&timada_orders::aggregator::Placed {
                    reference: "K7Q4-MZP2".to_owned(),
                    cart_id: "cart-1".to_owned(),
                    email: "ada@example.com".to_owned(),
                    shipping: ShippingAddress::default(),
                    note: None,
                    lines: Vec::new(),
                    currency: "EUR".to_owned(),
                    total_cents: 1000,
                    payment: Payment::default(),
                    supplier: Some(ALIEXPRESS_SUPPLIER.to_owned()),
                    placed_by: "storefront".to_owned(),
                });
            if paid {
                write.event(&timada_orders::aggregator::Paid {
                    payment_id: String::new(),
                    amount_cents: 1000,
                    paid_by: "timada-dashboard".to_owned(),
                });
            }
            // What the forwarder writes before it asks AliExpress.
            write.event(&timada_orders::aggregator::SupplierOrderStarted {
                supplier: ALIEXPRESS_SUPPLIER.to_owned(),
                started_by: ALIEXPRESS_ACTOR.to_owned(),
            });
            write.commit(&self.store).await.unwrap();
            let forwards = timada_aliexpress::Command::new(self.store.clone(), TENANT);
            let id = forwards
                .request_forward(ALIEXPRESS_ACTOR, ORDER, false)
                .await
                .unwrap();
            assert!(forwards.start_forward(ALIEXPRESS_ACTOR, &id).await.unwrap());
            forwards
                .record_placed(
                    ALIEXPRESS_ACTOR,
                    &id,
                    Placement {
                        ae_order_ids: ae_order_ids.iter().map(|id| (*id).to_owned()).collect(),
                        items: Vec::new(),
                        pay_requested: true,
                        complete: true,
                    },
                )
                .await
                .unwrap();
            id
        }

        fn ships(&self, ae_order_id: &str, parcel: &str) {
            self.stand_in
                .shipped
                .lock()
                .unwrap()
                .insert(ae_order_id.to_owned(), parcel.to_owned());
        }

        fn goes_down(&self) {
            self.stand_in.down.store(true, Ordering::SeqCst);
        }

        fn closes(&self, ae_order_id: &str) {
            self.stand_in
                .closed
                .lock()
                .unwrap()
                .push(ae_order_id.to_owned());
        }

        fn reads(&self) -> usize {
            self.stand_in.reads.load(Ordering::SeqCst)
        }

        /// One read on a node whose store is `store`.
        async fn check_on<E: Executor + Clone>(
            &self,
            store: &E,
            forward_id: &str,
            now: u64,
        ) -> anyhow::Result<CheckResult> {
            self.aliexpress
                .check_forward(store, &self.tenant, &self.company, forward_id, None, now)
                .await
        }

        /// One read by a node that does not stop.
        async fn check(&self, forward_id: &str) -> CheckResult {
            self.check_on(&self.store, forward_id, crate::auth::now())
                .await
                .unwrap()
        }

        fn stopping_at(&self, event: &'static str) -> StopsAt {
            StopsAt {
                store: self.store.clone(),
                event,
            }
        }

        fn orders(&self) -> timada_orders::Command<evento::Turso> {
            timada_orders::Command::new(self.store.clone(), TENANT)
        }

        async fn order(&self) -> OrderState {
            self.orders().load_order(ORDER).await.unwrap().unwrap()
        }

        async fn forward(&self, forward_id: &str) -> ForwardState {
            timada_aliexpress::Command::new(self.store.clone(), TENANT)
                .load_forward_by_id(forward_id)
                .await
                .unwrap()
                .unwrap()
        }
    }

    #[tokio::test]
    async fn an_order_is_fulfilled_once_every_parcel_is_known() {
        let shop = Shop::new().await;
        let id = shop.placed(&["81", "82"]).await;

        shop.ships("81", "LP1");
        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: false }
        );
        assert_eq!(shop.order().await.status, OrderStatus::Paid);
        let forward = shop.forward(&id).await;
        assert_eq!(forward.status, ForwardStatus::Placed);
        assert_eq!(forward.tracking(), vec!["LP1"]);

        shop.ships("82", "LP2");
        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: true }
        );
        let order = shop.order().await;
        assert_eq!(order.status, OrderStatus::Fulfilled);
        assert_eq!(order.tracking.as_deref(), Some("LP1, LP2"));
        let forward = shop.forward(&id).await;
        assert_eq!(forward.status, ForwardStatus::Done);
        assert_eq!(forward.tracking(), vec!["LP1", "LP2"]);
        assert_eq!(forward.fulfilled_tracking.as_deref(), Some("LP1, LP2"));

        assert_eq!(shop.check(&id).await, CheckResult::Skipped);
    }

    /// Issue #375: stopped between the order's `Fulfilled` and the forward's
    /// `Done`. The forward is closed as the lost run would have closed it,
    /// without AliExpress and however much later.
    #[tokio::test]
    async fn a_node_stopping_after_fulfilling_leaves_the_forward_to_be_closed() {
        let shop = Shop::new().await;
        let id = shop.placed(&["81", "82"]).await;
        shop.ships("81", "LP1");
        shop.ships("82", "LP2");
        let now = crate::auth::now();

        let stopping = shop.stopping_at("Done");
        assert!(shop.check_on(&stopping, &id, now).await.is_err());
        let order = shop.order().await;
        assert_eq!(order.status, OrderStatus::Fulfilled);
        assert_eq!(order.tracking.as_deref(), Some("LP1, LP2"));
        assert_eq!(shop.forward(&id).await.status, ForwardStatus::Placed);

        shop.goes_down();
        let reads = shop.reads();
        let later = now + timada_aliexpress::FORWARD_WATCH_SECS + 1;
        assert_eq!(
            shop.check_on(&shop.store, &id, later).await.unwrap(),
            CheckResult::Done
        );
        assert_eq!(shop.reads(), reads, "nothing left to ask AliExpress");
        let forward = shop.forward(&id).await;
        assert_eq!(forward.status, ForwardStatus::Done);
        assert_eq!(forward.fulfilled_tracking.as_deref(), Some("LP1, LP2"));
        assert_eq!(forward.tracking(), vec!["LP1", "LP2"]);
        for order in &forward.ae_orders {
            assert_eq!(order.status, "WAIT_BUYER_ACCEPT_GOODS");
            assert_eq!(order.logistics_status, "SELLER_SEND_GOODS");
        }
    }

    /// Stopped between the reads and the order's `Fulfilled`: the next read
    /// fulfils it from the reads recorded, whether AliExpress answers or not.
    #[tokio::test]
    async fn a_node_stopping_before_fulfilling_leaves_the_order_to_be_fulfilled() {
        let shop = Shop::new().await;
        let id = shop.placed(&["81"]).await;
        shop.ships("81", "LP1");

        let stopping = shop.stopping_at("Fulfilled");
        assert!(
            shop.check_on(&stopping, &id, crate::auth::now())
                .await
                .is_err()
        );
        assert_eq!(shop.order().await.status, OrderStatus::Paid);
        assert_eq!(shop.forward(&id).await.tracking(), vec!["LP1"]);

        shop.goes_down();
        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: true }
        );
        assert_eq!(shop.order().await.tracking.as_deref(), Some("LP1"));
        let forward = shop.forward(&id).await;
        assert_eq!(forward.status, ForwardStatus::Done);
        assert_eq!(forward.fulfilled_tracking.as_deref(), Some("LP1"));
    }

    /// The merchant fulfilled it with their own parcel: nothing says it came
    /// from AliExpress.
    #[tokio::test]
    async fn an_order_fulfilled_by_hand_closes_the_forward_without_a_tracking() {
        let shop = Shop::new().await;
        let id = shop.placed(&["81"]).await;
        shop.ships("81", "LP1");
        shop.orders()
            .fulfil_order("user-alice", ORDER, Some("HAND1".to_owned()))
            .await
            .unwrap();

        let reads = shop.reads();
        assert_eq!(shop.check(&id).await, CheckResult::Done);
        assert_eq!(shop.reads(), reads);
        let forward = shop.forward(&id).await;
        assert_eq!(forward.status, ForwardStatus::Done);
        assert_eq!(forward.fulfilled_tracking, None);
        assert_eq!(shop.order().await.tracking.as_deref(), Some("HAND1"));
    }

    /// The parcels are known before the money is taken (the capture
    /// failed, say): the order waits for it, then is fulfilled.
    #[tokio::test]
    async fn a_held_order_is_fulfilled_once_its_money_is_taken() {
        let shop = Shop::new().await;
        let id = shop.placed_with(&["81"], false).await;
        shop.ships("81", "LP1");

        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: false }
        );
        assert_eq!(shop.order().await.status, OrderStatus::Authorized);
        assert_eq!(shop.forward(&id).await.status, ForwardStatus::Placed);

        shop.orders()
            .request_capture("timada-dashboard", ORDER)
            .await
            .unwrap();
        shop.orders()
            .record_payment("timada-dashboard", ORDER, "", 1000)
            .await
            .unwrap();
        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: true }
        );
        assert_eq!(shop.order().await.status, OrderStatus::Fulfilled);
    }

    /// AliExpress closed every order of it, shipping nothing: the shop's
    /// order is to be cancelled (the payments sweep gives its money back),
    /// then the forward closes like any cancelled order's.
    #[tokio::test]
    async fn an_order_aliexpress_closed_is_cancelled() {
        let shop = Shop::new().await;
        let id = shop.placed_with(&["81", "82"], false).await;
        shop.closes("81");

        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: false }
        );
        let order = shop.order().await;
        assert!(order.pending_cancellation.is_none(), "one of two closed");
        assert!(shop.forward(&id).await.ae_orders[0].closed);

        shop.closes("82");
        shop.check(&id).await;
        let order = shop.order().await;
        let asked = order
            .pending_cancellation
            .expect("its cancellation is asked");
        assert_eq!(asked.reason, CLOSED_BY_ALIEXPRESS);
        assert_eq!(asked.requested_by, ALIEXPRESS_ACTOR);
        assert!(shop.forward(&id).await.all_closed());

        // The payments sweep gave the hold back: the forward closes.
        shop.orders()
            .record_release("timada-dashboard", ORDER, "", "")
            .await
            .unwrap();
        assert_eq!(shop.check(&id).await, CheckResult::Done);
    }

    /// The push is signed: an order it says AliExpress closed is closed even
    /// while AliExpress cannot be read.
    #[tokio::test]
    async fn a_closing_push_stands_while_aliexpress_is_unreachable() {
        let shop = Shop::new().await;
        let id = shop.placed_with(&["81"], false).await;
        shop.goes_down();

        shop.aliexpress
            .check_forward(
                &shop.store,
                &shop.tenant,
                &shop.company,
                &id,
                Some("81"),
                crate::auth::now(),
            )
            .await
            .unwrap();
        assert!(shop.forward(&id).await.all_closed());
        assert!(shop.order().await.pending_cancellation.is_some());
    }

    /// The hold went back while AliExpress had the order: kept open for the
    /// merchant. Once every parcel is known nothing a read finds changes
    /// anything, so the forward closes and AliExpress is asked no more.
    #[tokio::test]
    async fn an_order_kept_open_without_its_hold_is_not_read_once_every_parcel_is_known() {
        let shop = Shop::new().await;
        let id = shop.placed_with(&["81"], false).await;
        let released = shop
            .orders()
            .record_release("timada-dashboard", ORDER, "", "The hold lapsed.")
            .await
            .unwrap();
        assert_eq!(released, timada_orders::Released::KeptOpen);
        shop.ships("81", "LP1");

        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: false }
        );
        let reads = shop.reads();
        assert_eq!(shop.check(&id).await, CheckResult::Done);
        assert_eq!(shop.reads(), reads, "nothing left to ask AliExpress");
        let order = shop.order().await;
        assert_eq!(
            order.status,
            OrderStatus::Authorized,
            "the merchant settles it"
        );
        assert_eq!(shop.forward(&id).await.fulfilled_tracking, None);
    }

    /// The money of an order kept open came in outside timada: paid, it is
    /// fulfilled from AliExpress's parcels, whether they arrive after (the
    /// next read) or arrived before and closed the forward (the saga).
    #[tokio::test]
    async fn an_order_settled_outside_timada_is_fulfilled_from_its_parcels() {
        let shop = Shop::new().await;
        let id = shop.placed_with(&["81"], false).await;
        shop.orders()
            .record_release("timada-dashboard", ORDER, "", "The hold lapsed.")
            .await
            .unwrap();
        // Nothing shipped yet: nothing to fulfil from.
        shop.orders()
            .settle_outside("user-alice", ORDER, None)
            .await
            .unwrap();
        assert!(
            !shop
                .aliexpress
                .fulfil_settled_order(&shop.store, TENANT, ORDER)
                .await
                .unwrap()
        );
        assert_eq!(shop.order().await.status, OrderStatus::Paid);
        // The parcel arrives: the read fulfils it, as any paid order.
        shop.ships("81", "LP1");
        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: true }
        );
        let order = shop.order().await;
        assert_eq!(order.status, OrderStatus::Fulfilled);
        assert_eq!(order.tracking.as_deref(), Some("LP1"));
        assert_eq!(shop.forward(&id).await.status, ForwardStatus::Done);

        // Every parcel known before the merchant spoke: the forward closed
        // with nothing to fulfil, so the settlement fulfils from it.
        let shop = Shop::new().await;
        let id = shop.placed_with(&["82"], false).await;
        shop.orders()
            .record_release("timada-dashboard", ORDER, "", "The hold lapsed.")
            .await
            .unwrap();
        shop.ships("82", "LP2");
        assert_eq!(
            shop.check(&id).await,
            CheckResult::Checked { fulfilled: false }
        );
        assert_eq!(shop.check(&id).await, CheckResult::Done);
        assert_eq!(shop.order().await.status, OrderStatus::Authorized);
        shop.orders()
            .settle_outside("user-alice", ORDER, Some("cash".to_owned()))
            .await
            .unwrap();
        assert!(
            shop.aliexpress
                .fulfil_settled_order(&shop.store, TENANT, ORDER)
                .await
                .unwrap()
        );
        let order = shop.order().await;
        assert_eq!(order.status, OrderStatus::Fulfilled);
        assert_eq!(order.tracking.as_deref(), Some("LP2"));
        assert_eq!(order.fulfilled_by.as_deref(), Some(ALIEXPRESS_ACTOR));
        // Told again (another node): nothing more.
        assert!(
            !shop
                .aliexpress
                .fulfil_settled_order(&shop.store, TENANT, ORDER)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_cancelled_order_closes_the_forward_without_a_tracking() {
        let shop = Shop::new().await;
        let id = shop.placed(&["81"]).await;
        let orders = shop.orders();
        orders
            .request_cancellation("user-alice", ORDER, "changed my mind".to_owned(), true)
            .await
            .unwrap();
        let refund = orders
            .request_refund("user-alice", ORDER, 1000)
            .await
            .unwrap();
        orders
            .record_refund("timada-dashboard", ORDER, "re_1", refund.amount_cents)
            .await
            .unwrap();
        orders.complete_cancellation(ORDER).await.unwrap();

        assert_eq!(shop.check(&id).await, CheckResult::Done);
        let forward = shop.forward(&id).await;
        assert_eq!(forward.status, ForwardStatus::Done);
        assert_eq!(forward.fulfilled_tracking, None);
    }

    #[tokio::test]
    async fn an_order_watched_long_enough_is_no_longer_read() {
        let shop = Shop::new().await;
        let id = shop.placed(&["81"]).await;

        let reads = shop.reads();
        let later = crate::auth::now() + timada_aliexpress::FORWARD_WATCH_SECS + 1;
        assert_eq!(
            shop.check_on(&shop.store, &id, later).await.unwrap(),
            CheckResult::Done
        );
        assert_eq!(shop.reads(), reads);
        assert_eq!(shop.order().await.status, OrderStatus::Paid);
        let forward = shop.forward(&id).await;
        assert_eq!(forward.status, ForwardStatus::Done);
        assert_eq!(forward.fulfilled_tracking, None);
    }
}
