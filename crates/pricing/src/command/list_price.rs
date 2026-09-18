use evento::Executor;
use timada_core::Money;

use crate::{aggregator::ProductPriceListed, error::PricingError};

use super::price_id;

#[derive(Debug, Clone)]
pub struct ListPrice {
    pub product_id: String,
    pub price_incl_tax: Money,
    pub vat_rate_bp: u16,
    pub eco_participation: Money,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Lists a product's price. The id is derived from the product id, so a
    /// second listing for the same product is rejected atomically by the store.
    pub async fn list_price(
        &self,
        cmd: ListPrice,
        routing_key: Option<String>,
    ) -> Result<String, PricingError> {
        let product_id = cmd.product_id.trim().to_owned();
        if product_id.is_empty() {
            return Err(PricingError::Required("product_id"));
        }
        if !cmd.price_incl_tax.is_positive() {
            return Err(PricingError::InvalidAmount);
        }
        if cmd.eco_participation.is_negative() {
            return Err(PricingError::NegativeAmount);
        }
        cmd.price_incl_tax.same_currency(&cmd.eco_participation)?;

        let id = price_id(&product_id);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&ProductPriceListed {
                product_id: product_id.clone(),
                price_incl_tax: cmd.price_incl_tax,
                vat_rate_bp: cmd.vat_rate_bp,
                eco_participation: cmd.eco_participation,
            })
            .commit(self.0)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(price_id = %id, %product_id, "product price listed");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(PricingError::AlreadyListed(product_id))
            }
            Err(err) => Err(err.into()),
        }
    }
}
