mod attach_installment_offer;
mod change_eco_participation;
mod change_price;
mod list_price;
mod withdraw_price;

use std::ops::Deref;

pub use list_price::ListPrice;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        EcoParticipationChanged, InstallmentOfferAttached, ProductPrice, ProductPriceChanged,
        ProductPriceListed, ProductPriceWithdrawn,
    },
    error::PricingError,
};

/// Deterministic price id: one price stream per product.
pub fn price_id(product_id: &str) -> String {
    timada_core::id::derived(&[product_id], "price")
}

pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<E: Executor> Command<E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<ProductPriceState>> {
        create_projection().load(id).execute(&self.0).await
    }

    /// Loads a price that must exist and not be withdrawn.
    async fn load_active(&self, id: impl Into<String>) -> Result<ProductPriceState, PricingError> {
        let Some(price) = self.load(id).await? else {
            return Err(PricingError::PriceNotFound);
        };
        if price.withdrawn {
            return Err(PricingError::PriceWithdrawn);
        }
        Ok(price)
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct ProductPriceState {
    pub id: String,
    pub currency: String,
    pub withdrawn: bool,
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, ProductPriceState> {
    Projection::new::<ProductPrice>()
        .handler(on_product_price_listed())
        .handler(on_product_price_withdrawn())
        .skip::<ProductPriceChanged>()
        .skip::<EcoParticipationChanged>()
        .skip::<InstallmentOfferAttached>()
        .strict()
}

#[evento::handler]
async fn on_product_price_listed(
    event: Event<ProductPriceListed>,
    row: &mut ProductPriceState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.currency = event.data.price_incl_tax.currency;
    Ok(())
}

#[evento::handler]
async fn on_product_price_withdrawn(
    _event: Event<ProductPriceWithdrawn>,
    row: &mut ProductPriceState,
) -> anyhow::Result<()> {
    row.withdrawn = true;
    Ok(())
}
