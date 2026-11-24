//! What the shop pays: for one of each thing it sells, and for a sale
//! whatever it sells. Nothing here is ever shown to a shopper; a margin is
//! worked out from it ([`Margin`](crate::Margin)).

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::{CostSet, MarginTiersSet, OwnCostsSet, SaleCostsSet, VariantCostsSet};
use crate::error::CatalogError;
use crate::margin::{MAX_FEE_BPS, MarginTiers, ProductCosts, ShopCosts};
use crate::value_object::{
    FEED_LABELS, MAX_AMOUNT_CENTS, VariantCost, normalize_currency, product_cost_id, sale_costs_id,
};

#[evento::projection(id = id, name = "timada-catalog/ProductCostState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ProductCostState {
    pub id: String,
    pub product_id: String,
    pub costs: ProductCosts,
}

pub fn product_cost_projection<E: Executor>() -> Projection<E, ProductCostState> {
    Projection::new::<crate::aggregator::ProductCost>()
        .handler(on_cost_set())
        .handler(on_variant_costs_set())
        .handler(on_own_costs_set())
        .strict()
}

#[evento::handler]
async fn on_cost_set(event: Event<CostSet>, state: &mut ProductCostState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.product_id = event.data.product_id.to_owned();
    state.costs.cost_cents = event.data.amount_cents;
    Ok(())
}

#[evento::handler]
async fn on_variant_costs_set(
    event: Event<VariantCostsSet>,
    state: &mut ProductCostState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.product_id = event.data.product_id.to_owned();
    for cost in event.data.costs.iter() {
        match cost.amount_cents {
            Some(amount_cents) => {
                state
                    .costs
                    .variants
                    .insert(cost.variant_id.to_owned(), amount_cents);
            }
            None => {
                state.costs.variants.remove(&cost.variant_id);
            }
        }
    }
    Ok(())
}

#[evento::handler]
async fn on_own_costs_set(
    event: Event<OwnCostsSet>,
    state: &mut ProductCostState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.product_id = event.data.product_id.to_owned();
    state.costs.own_shipping_cents = event.data.shipping_cents;
    state.costs.own_import_cents = event.data.import_cents;
    Ok(())
}

/// What the shop pays for the things a product sells, as the store has
/// it; `tenant` is the one the stream is in. Nothing said is the default.
pub async fn load_product_costs<E: Executor>(
    executor: &E,
    tenant: &str,
    product_id: &str,
) -> anyhow::Result<ProductCosts> {
    Ok(product_cost_projection()
        .load(product_cost_id(product_id))
        .tenant(tenant)
        .execute(executor)
        .await?
        .map(|state| state.costs)
        .unwrap_or_default())
}

#[evento::projection(id = id, name = "timada-catalog/SaleCostsState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct SaleCostsState {
    pub id: String,
    /// What a sale costs the shop beyond the goods; `None` while it said
    /// nothing.
    pub costs: Option<ShopCosts>,
    /// How its margins are graded; `None` is [`MarginTiers::default`].
    pub tiers: Option<MarginTiers>,
}

impl SaleCostsState {
    /// How the shop grades its margins, as it set them or as a shop that
    /// set nothing does.
    pub fn tiers(&self) -> MarginTiers {
        self.tiers.unwrap_or_default()
    }
}

pub fn sale_costs_projection<E: Executor>() -> Projection<E, SaleCostsState> {
    Projection::new::<crate::aggregator::SaleCosts>()
        .handler(on_sale_costs_set())
        .handler(on_margin_tiers_set())
        .strict()
}

#[evento::handler]
async fn on_sale_costs_set(
    event: Event<SaleCostsSet>,
    state: &mut SaleCostsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.costs = Some(ShopCosts {
        currency: event.data.currency.to_owned(),
        shipping_cents: event.data.shipping_cents,
        import_cents: event.data.import_cents,
        fixed_cents: event.data.fixed_cents,
        payment_fee_bps: event.data.payment_fee_bps,
        payment_fee_cents: event.data.payment_fee_cents,
    });
    Ok(())
}

#[evento::handler]
async fn on_margin_tiers_set(
    event: Event<MarginTiersSet>,
    state: &mut SaleCostsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.tiers = Some(MarginTiers {
        medium_from_bps: event.data.medium_from_bps,
        high_from_bps: event.data.high_from_bps,
        feed_label: event.data.feed_label,
    });
    Ok(())
}

/// The shop's sale costs as the store has them; `tenant` is the one the
/// stream is in. A shop that said nothing has the default state.
pub async fn load_sale_costs<E: Executor>(
    executor: &E,
    tenant: &str,
) -> anyhow::Result<SaleCostsState> {
    Ok(sale_costs_projection()
        .load(sale_costs_id())
        .tenant(tenant)
        .execute(executor)
        .await?
        .unwrap_or_default())
}

fn require_amount(amount_cents: u64) -> Result<(), CatalogError> {
    if amount_cents > MAX_AMOUNT_CENTS {
        return Err(CatalogError::InvalidAmount);
    }
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Says what the shop pays for one of a product sold as is, in the
    /// product's currency; `None` forgets it. Refused on a product sold
    /// through its combinations, each of which has its own. The same is a
    /// no-op.
    pub async fn set_cost(
        &self,
        actor: &str,
        product_id: &str,
        amount_cents: Option<u64>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        amount_cents.map(require_amount).transpose()?;
        let product = self.require_editable(product_id).await?;
        if !product.variants.is_empty() {
            return Err(CatalogError::CostOnVariants);
        }
        let state = self.load_product_cost_state(product_id).await?;
        if state.as_ref().and_then(|state| state.costs.cost_cents) == amount_cents {
            return Ok(());
        }
        write_product_cost(state.as_ref(), product_id)?
            .routing_key_opt(routing_key)
            .event(&CostSet {
                product_id: product_id.to_owned(),
                amount_cents,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Says what the shop pays for one of several combinations at once, in
    /// one event; `None` forgets a cost. Those already at their cost are
    /// left out, and none left is a no-op; whether any was set. Every one
    /// must be a combination of the product, at a valid amount, or nothing
    /// is set.
    pub async fn set_variant_costs(
        &self,
        actor: &str,
        product_id: &str,
        costs: &[VariantCost],
        routing_key: Option<String>,
    ) -> Result<bool, CatalogError> {
        for cost in costs {
            cost.amount_cents.map(require_amount).transpose()?;
        }
        let product = self.require_editable(product_id).await?;
        let state = self.load_product_cost_state(product_id).await?;
        let known = |variant_id: &str| {
            state
                .as_ref()
                .and_then(|state| state.costs.cost_of(Some(variant_id)))
        };
        let mut changed: Vec<VariantCost> = Vec::with_capacity(costs.len());
        for cost in costs {
            if product.variant(&cost.variant_id).is_none() {
                return Err(CatalogError::VariantNotFound);
            }
            if known(&cost.variant_id) != cost.amount_cents
                && !changed
                    .iter()
                    .any(|known| known.variant_id == cost.variant_id)
            {
                changed.push(cost.clone());
            }
        }
        if changed.is_empty() {
            return Ok(false);
        }
        write_product_cost(state.as_ref(), product_id)?
            .routing_key_opt(routing_key)
            .event(&VariantCostsSet {
                product_id: product_id.to_owned(),
                costs: changed,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(true)
    }

    /// Says what the shop pays for one of everything the product sells:
    /// the product itself when it is sold as is, each of its combinations
    /// otherwise. What costing many products at once does to each.
    pub async fn set_every_cost(
        &self,
        actor: &str,
        product_id: &str,
        amount_cents: Option<u64>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if product.variants.is_empty() {
            return self
                .set_cost_opt(actor, product_id, amount_cents, routing_key)
                .await;
        }
        let costs: Vec<VariantCost> = product
            .variants
            .iter()
            .map(|variant| VariantCost {
                variant_id: variant.id.clone(),
                amount_cents,
            })
            .collect();
        self.set_variant_costs_opt(actor, product_id, &costs, routing_key)
            .await?;
        Ok(())
    }

    /// Says what sending one and importing one cost the shop for this
    /// product, where they are not the shop's usual; `None` goes back to
    /// the shop's. The same is a no-op.
    pub async fn set_own_costs(
        &self,
        actor: &str,
        product_id: &str,
        shipping_cents: Option<u64>,
        import_cents: Option<u64>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        shipping_cents.map(require_amount).transpose()?;
        import_cents.map(require_amount).transpose()?;
        self.require_editable(product_id).await?;
        let state = self.load_product_cost_state(product_id).await?;
        let known = state
            .as_ref()
            .map(|state| (state.costs.own_shipping_cents, state.costs.own_import_cents))
            .unwrap_or_default();
        if known == (shipping_cents, import_cents) {
            return Ok(());
        }
        write_product_cost(state.as_ref(), product_id)?
            .routing_key_opt(routing_key)
            .event(&OwnCostsSet {
                product_id: product_id.to_owned(),
                shipping_cents,
                import_cents,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Says what a sale costs the shop beyond the goods. The same is a
    /// no-op.
    pub async fn set_sale_costs(
        &self,
        actor: &str,
        costs: ShopCosts,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let currency = normalize_currency(&costs.currency)?;
        for amount in [
            costs.shipping_cents,
            costs.import_cents,
            costs.fixed_cents,
            costs.payment_fee_cents,
        ] {
            require_amount(amount)?;
        }
        if costs.payment_fee_bps > MAX_FEE_BPS {
            return Err(CatalogError::InvalidFee);
        }
        let costs = ShopCosts { currency, ..costs };
        let state = self.load_sale_costs().await?;
        if state.costs.as_ref() == Some(&costs) {
            return Ok(());
        }
        self.write_sale_costs(&state)?
            .routing_key_opt(routing_key)
            .event(&SaleCostsSet {
                currency: costs.currency,
                shipping_cents: costs.shipping_cents,
                import_cents: costs.import_cents,
                fixed_cents: costs.fixed_cents,
                payment_fee_bps: costs.payment_fee_bps,
                payment_fee_cents: costs.payment_fee_cents,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Says from which share of the price a margin is medium and high, and
    /// which label of the feed carries the tier. A high margin starts above
    /// a medium one, both under the whole price. The same is a no-op.
    pub async fn set_margin_tiers(
        &self,
        actor: &str,
        tiers: MarginTiers,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if tiers.medium_from_bps == 0
            || tiers.medium_from_bps >= tiers.high_from_bps
            || tiers.high_from_bps >= 10_000
            || tiers
                .feed_label
                .is_some_and(|label| usize::from(label) >= FEED_LABELS)
        {
            return Err(CatalogError::InvalidMarginTiers);
        }
        let state = self.load_sale_costs().await?;
        if state.tiers() == tiers {
            return Ok(());
        }
        // One label says one thing: not the tier and "out of ads" at once.
        if tiers.feed_label.is_some() && tiers.feed_label == self.load_feed_ads_label().await? {
            return Err(CatalogError::FeedLabelTaken);
        }
        self.write_sale_costs(&state)?
            .routing_key_opt(routing_key)
            .event(&MarginTiersSet {
                medium_from_bps: tiers.medium_from_bps,
                high_from_bps: tiers.high_from_bps,
                feed_label: tiers.feed_label,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

/// A write to a product's costs at the version `state` was read at: the
/// stream's first when the shop said nothing of the product yet.
fn write_product_cost(
    state: Option<&ProductCostState>,
    product_id: &str,
) -> Result<evento::WriteBuilder, CatalogError> {
    match state {
        Some(state) => Ok(state.write()?),
        None => {
            let mut write = evento::append(product_cost_id(product_id));
            write.original_version(0);
            Ok(write)
        }
    }
}

impl<E: Executor> super::Command<E> {
    /// What the shop pays for the things a product sells; nothing said is
    /// the default.
    pub async fn load_product_costs(&self, product_id: &str) -> anyhow::Result<ProductCosts> {
        load_product_costs(&self.executor, &self.tenant, product_id).await
    }

    async fn load_product_cost_state(
        &self,
        product_id: &str,
    ) -> anyhow::Result<Option<ProductCostState>> {
        product_cost_projection()
            .load(product_cost_id(product_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// What a sale costs the shop and how it grades its margins; the
    /// default state for a shop that said nothing.
    pub async fn load_sale_costs(&self) -> anyhow::Result<SaleCostsState> {
        load_sale_costs(&self.executor, &self.tenant).await
    }

    /// A write to the shop's sale costs at the version `state` was read at:
    /// the stream's first when it said nothing yet.
    fn write_sale_costs(
        &self,
        state: &SaleCostsState,
    ) -> Result<evento::WriteBuilder, CatalogError> {
        if state.id.is_empty() {
            let mut write = evento::append(sale_costs_id());
            write.original_version(0);
            Ok(write)
        } else {
            Ok(state.write()?)
        }
    }
}
