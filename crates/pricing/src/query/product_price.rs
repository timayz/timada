//! The price block of the product page: TTC / HT, éco-participation and the
//! installment estimate. Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::{
    aggregator::{
        EcoParticipationChanged, InstallmentOfferAttached, ProductPrice, ProductPriceChanged,
        ProductPriceListed, ProductPriceWithdrawn,
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
}

impl ProductPriceView {
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
