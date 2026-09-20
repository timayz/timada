mod attach_installment_offer;
mod change_eco_participation;
mod change_price;
mod currency_price;
mod list_price;
mod withdraw_price;

use std::ops::Deref;

pub use list_price::ListPrice;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        CurrencyPriceRemoved, CurrencyPriceSet, EcoParticipationChanged, InstallmentOfferAttached,
        ProductPrice, ProductPriceChanged, ProductPriceListed, ProductPriceWithdrawn,
    },
    error::PricingError,
};

/// Deterministic price id: one price stream per product.
pub fn price_id(product_id: &str) -> String {
    timada_core::id::derived(&[product_id], "price")
}

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<ProductPriceState>> {
        create_projection().load(id).execute(self.0).await
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
    /// The currency the product was listed in.
    pub currency: String,
    pub withdrawn: bool,
    /// `(currency, minor units)` of every price, the listed one first.
    pub prices: Vec<(String, i64)>,
}

impl ProductPriceState {
    /// The tax-inclusive price in `currency`, in minor units.
    pub fn price_in(&self, currency: &str) -> Option<i64> {
        self.prices
            .iter()
            .find(|(known, _)| known == currency)
            .map(|(_, minor)| *minor)
    }

    fn set_price(&mut self, price: &timada_core::Money) {
        match self.prices.iter_mut().find(|(c, _)| *c == price.currency) {
            Some(known) => known.1 = price.minor,
            None => self.prices.push((price.currency.clone(), price.minor)),
        }
    }
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, ProductPriceState> {
    Projection::new::<ProductPrice>()
        .handler(on_product_price_listed())
        .handler(on_product_price_withdrawn())
        .handler(on_product_price_changed())
        .handler(on_currency_price_set())
        .handler(on_currency_price_removed())
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
    row.set_price(&event.data.price_incl_tax);
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

#[evento::handler]
async fn on_product_price_changed(
    event: Event<ProductPriceChanged>,
    row: &mut ProductPriceState,
) -> anyhow::Result<()> {
    row.set_price(&event.data.price_incl_tax);
    Ok(())
}

#[evento::handler]
async fn on_currency_price_set(
    event: Event<CurrencyPriceSet>,
    row: &mut ProductPriceState,
) -> anyhow::Result<()> {
    row.set_price(&event.data.price_incl_tax);
    Ok(())
}

#[evento::handler]
async fn on_currency_price_removed(
    event: Event<CurrencyPriceRemoved>,
    row: &mut ProductPriceState,
) -> anyhow::Result<()> {
    row.prices
        .retain(|(currency, _)| *currency != event.data.currency);
    Ok(())
}
