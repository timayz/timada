//! The price block of the product page: TTC / HT, éco-participation and the
//! installment estimate. Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::{
    aggregator::{
        CurrencyPriceRemoved, CurrencyPriceSet, EcoParticipationChanged, InstallmentOfferAttached,
        ProductPrice, ProductPriceChanged, ProductPriceListed, ProductPriceWithdrawn,
    },
    value_object::InstallmentOffer,
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct ProductPriceView {
    pub product_id: String,
    pub price_incl_tax: Money,
    pub price_excl_tax: Money,
    pub vat_rate_bp: u16,
    pub eco_participation: Money,
    pub installment: Option<InstallmentOffer>,
    /// Per-installment amount, `(price + fee) / count`, rounded down.
    pub installment_amount: Option<Money>,
    pub withdrawn: bool,
    /// The prices the operator set in other currencies, in the order they
    /// were first set. `price_incl_tax` stays the listed one.
    pub currency_prices: Vec<Money>,
}

/// What a product costs in one currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceIn {
    pub price_incl_tax: Money,
    pub price_excl_tax: Money,
    /// The éco-participation is a contribution of the listed currency: it
    /// shows nowhere else.
    pub eco_participation: Option<Money>,
    /// An instalment offer is priced in one currency (its fee's): it shows
    /// nowhere else.
    pub installment: Option<(InstallmentOffer, Money)>,
}

impl ProductPriceView {
    /// The currency the product was listed in.
    pub fn listed_currency(&self) -> &str {
        &self.price_incl_tax.currency
    }

    /// Every currency the product has a price in, the listed one first.
    pub fn currencies(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.listed_currency())
            .chain(self.currency_prices.iter().map(|p| p.currency.as_str()))
    }

    /// The product's price in `currency`; `None` when it is not sold in it
    /// (no price set there, or the price withdrawn).
    pub fn price_in(&self, currency: &str) -> Option<PriceIn> {
        if self.withdrawn {
            return None;
        }
        let listed = self.listed_currency() == currency;
        let price_incl_tax = if listed {
            self.price_incl_tax.clone()
        } else {
            self.currency_prices
                .iter()
                .find(|p| p.currency == currency)?
                .clone()
        };
        let installment = self
            .installment
            .as_ref()
            .filter(|offer| offer.fee.currency == currency)
            .and_then(|offer| {
                let amount = price_incl_tax
                    .checked_add(&offer.fee)
                    .ok()?
                    .divided_by(u32::from(offer.count));
                Some((offer.clone(), amount))
            });
        Some(PriceIn {
            price_excl_tax: price_incl_tax.excl_tax(self.vat_rate_bp),
            eco_participation: listed.then(|| self.eco_participation.clone()),
            installment,
            price_incl_tax,
        })
    }

    fn recompute(&mut self) -> anyhow::Result<()> {
        self.price_excl_tax = self.price_incl_tax.excl_tax(self.vat_rate_bp);
        self.installment_amount = match &self.installment {
            Some(offer) => Some(
                self.price_incl_tax
                    .checked_add(&offer.fee)?
                    .divided_by(u32::from(offer.count)),
            ),
            None => None,
        };
        Ok(())
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, ProductPriceView> {
    Projection::new::<ProductPrice>()
        .handler(on_product_price_listed())
        .handler(on_product_price_changed())
        .handler(on_eco_participation_changed())
        .handler(on_installment_offer_attached())
        .handler(on_product_price_withdrawn())
        .handler(on_currency_price_set())
        .handler(on_currency_price_removed())
        // `currency_prices` joined the snapshot.
        .revision(1)
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<ProductPriceView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_product_price_listed(
    event: Event<ProductPriceListed>,
    row: &mut ProductPriceView,
) -> anyhow::Result<()> {
    row.product_id = event.data.product_id;
    row.price_incl_tax = event.data.price_incl_tax;
    row.vat_rate_bp = event.data.vat_rate_bp;
    row.eco_participation = event.data.eco_participation;
    row.recompute()
}

#[evento::handler]
async fn on_product_price_changed(
    event: Event<ProductPriceChanged>,
    row: &mut ProductPriceView,
) -> anyhow::Result<()> {
    row.price_incl_tax = event.data.price_incl_tax;
    row.recompute()
}

#[evento::handler]
async fn on_eco_participation_changed(
    event: Event<EcoParticipationChanged>,
    row: &mut ProductPriceView,
) -> anyhow::Result<()> {
    row.eco_participation = event.data.eco_participation;
    Ok(())
}

#[evento::handler]
async fn on_installment_offer_attached(
    event: Event<InstallmentOfferAttached>,
    row: &mut ProductPriceView,
) -> anyhow::Result<()> {
    row.installment = Some(event.data.offer);
    row.recompute()
}

#[evento::handler]
async fn on_product_price_withdrawn(
    _event: Event<ProductPriceWithdrawn>,
    row: &mut ProductPriceView,
) -> anyhow::Result<()> {
    row.withdrawn = true;
    Ok(())
}

#[evento::handler]
async fn on_currency_price_set(
    event: Event<CurrencyPriceSet>,
    row: &mut ProductPriceView,
) -> anyhow::Result<()> {
    let price = event.data.price_incl_tax;
    match row
        .currency_prices
        .iter_mut()
        .find(|known| known.currency == price.currency)
    {
        Some(known) => *known = price,
        None => row.currency_prices.push(price),
    }
    Ok(())
}

#[evento::handler]
async fn on_currency_price_removed(
    event: Event<CurrencyPriceRemoved>,
    row: &mut ProductPriceView,
) -> anyhow::Result<()> {
    row.currency_prices
        .retain(|price| price.currency != event.data.currency);
    Ok(())
}
