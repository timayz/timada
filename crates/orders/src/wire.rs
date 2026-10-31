//! What the dashboard answers a storefront over HTTP about one cart or one
//! order (`/store/carts/*`, `/store/orders/{order_id}`), as both ends encode
//! it. A storefront reads no cart or order stream itself: its key lets it
//! read the catalog and how the shop sells, and the dashboard answers for
//! the one cart or order whose id it names, in the key's tenant.

use evento::WriteError;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{CartState, OrderError, PaymentAnswer, Quote, ShippingMethod};

/// `GET /store/carts/{cart_id}`: the cart, priced from the catalog as it
/// stands, and the ways the shop ships.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartAnswer {
    pub cart: CartState,
    pub quote: Quote,
    pub shipping_methods: Vec<ShippingMethod>,
}

/// `POST /store/carts`: the cart just started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub id: String,
}

/// `POST /store/carts/{cart_id}/discount`: the code to put on the cart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscountCode {
    pub code: String,
}

/// A request to pay and its answer (`POST /store/carts/{cart_id}/checkout`,
/// `GET /store/carts/{cart_id}/requests/{request_id}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAnswer {
    pub request_id: String,
    /// `None` while the dashboard has not answered it.
    pub answer: Option<PaymentAnswer>,
}

/// A refusal to show the shopper, as it travels: the code of
/// [`OrderError::code`] and what some refusals carry beside it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cents: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
}

impl Refusal {
    /// The refusal as it travels; `None` for an infrastructure failure,
    /// which is not told to the shopper.
    pub fn of(err: &OrderError) -> Option<Self> {
        let code = err.code().filter(|_| err.is_user_facing())?;
        let mut refusal = Refusal {
            error: code.to_owned(),
            ..Refusal::default()
        };
        match err {
            OrderError::Unavailable { product_id } => {
                refusal.product_id = Some(product_id.clone());
            }
            OrderError::OutOfStock {
                product_id,
                available,
            } => {
                refusal.product_id = Some(product_id.clone());
                refusal.available = Some(*available);
            }
            OrderError::PriceChanged { total_cents } => {
                refusal.total_cents = Some(*total_cents);
            }
            OrderError::DuplicatePayment { order_id } => {
                refusal.order_id = Some(order_id.clone());
            }
            _ => {}
        }
        Some(refusal)
    }

    /// The refusal back as the error a page words. A code this crate does
    /// not know is an [`OrderError::Other`], which a page treats as a
    /// failure rather than wording it.
    pub fn into_error(self) -> OrderError {
        let Refusal {
            error,
            product_id,
            available,
            total_cents,
            order_id,
        } = self;
        if let Some(code) = shopper_code_of(&error) {
            return OrderError::DiscountRefused { code };
        }
        match error.as_str() {
            "cart_not_found" => OrderError::CartNotFound,
            "cart_checked_out" => OrderError::CheckedOut,
            "invalid_quantity" => OrderError::InvalidQuantity,
            "too_many_lines" => OrderError::TooManyLines,
            "product_unavailable" => OrderError::Unavailable {
                product_id: product_id.unwrap_or_default(),
            },
            "out_of_stock" => OrderError::OutOfStock {
                product_id: product_id.unwrap_or_default(),
                available: available.unwrap_or_default(),
            },
            "variant_required" => OrderError::VariantRequired,
            "variant_not_found" => OrderError::VariantNotFound,
            "mixed_currencies" => OrderError::MixedCurrencies,
            "empty_cart" => OrderError::EmptyCart,
            "price_changed" => OrderError::PriceChanged {
                total_cents: total_cents.unwrap_or_default(),
            },
            "payment_not_found" => OrderError::PaymentNotFound,
            "amount_mismatch" => OrderError::AmountMismatch,
            "duplicate_payment" => OrderError::DuplicatePayment {
                order_id: order_id.unwrap_or_default(),
            },
            "invalid_email" => OrderError::InvalidEmail,
            "invalid_vat_number" => OrderError::InvalidVatNumber,
            "business_buyers_refused" => OrderError::BusinessBuyersRefused,
            "invalid_ship_name" => OrderError::InvalidName,
            "invalid_address" => OrderError::InvalidAddress,
            "invalid_country" => OrderError::InvalidCountry,
            "invalid_phone" => OrderError::InvalidPhone,
            "invalid_note" => OrderError::InvalidNote,
            "order_not_found" => OrderError::OrderNotFound,
            "shipping_method_not_found" => OrderError::ShippingMethodNotFound,
            "shipping_unavailable" => OrderError::ShippingUnavailable,
            crate::PAYMENTS_UNAVAILABLE
            | crate::INVOICING_NOT_CONFIGURED
            | crate::OSS_REQUIRED_FOR_CURRENCY => OrderError::PaymentRefused { code: error },
            code if code == timada_core::WRITES_PAUSED => OrderError::Write(
                WriteError::Unavailable(anyhow::anyhow!("the shop is not taking writes right now")),
            ),
            _ => OrderError::Other(anyhow::anyhow!("the dashboard refused: {error}")),
        }
    }
}

/// The `&'static str` of a discount refusal code a shopper reads, or `None`
/// for a code that is not one.
fn shopper_code_of(code: &str) -> Option<&'static str> {
    timada_discounts::SHOPPER_CODES
        .iter()
        .copied()
        .find(|known| *known == code)
}

/// A discount refusal code a shopper reads, one of
/// [`timada_discounts::SHOPPER_CODES`]. Spelt through this alias in
/// [`Quote::discount_refused`] so serde's derive does not take the `&str`
/// for borrowed input (it reads the type's spelling, not what
/// [`shopper_code`] does).
pub type ShopperCode = &'static str;

/// Deserializes [`Quote::discount_refused`]: a shopper code back to the
/// crate's own `&'static str`, anything else refused.
pub fn shopper_code<'de, D>(deserializer: D) -> Result<Option<&'static str>, D::Error>
where
    D: Deserializer<'de>,
{
    let code: Option<String> = Option::deserialize(deserializer)?;
    match code {
        None => Ok(None),
        Some(code) => shopper_code_of(&code).map(Some).ok_or_else(|| {
            serde::de::Error::custom(format!("{code} is not a discount refusal a shopper reads"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_travels_with_what_it_carries() {
        let err = OrderError::OutOfStock {
            product_id: "mug".into(),
            available: 2,
        };
        let refusal = Refusal::of(&err).unwrap();
        let json = serde_json::to_string(&refusal).unwrap();
        assert_eq!(
            json,
            r#"{"error":"out_of_stock","product_id":"mug","available":2}"#
        );
        let back: Refusal = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back.into_error(),
            OrderError::OutOfStock { product_id, available: 2 } if product_id == "mug"
        ));
        assert!(Refusal::of(&OrderError::Other(anyhow::anyhow!("boom"))).is_none());
    }

    #[test]
    fn every_code_a_shopper_reads_comes_back_as_itself() {
        let errors = [
            OrderError::CartNotFound,
            OrderError::CheckedOut,
            OrderError::InvalidQuantity,
            OrderError::EmptyCart,
            OrderError::PriceChanged { total_cents: 1250 },
            OrderError::ShippingUnavailable,
            OrderError::DiscountRefused {
                code: timada_discounts::DISCOUNT_EXHAUSTED,
            },
            OrderError::PaymentRefused {
                code: crate::PAYMENTS_UNAVAILABLE.into(),
            },
        ];
        for err in errors {
            let code = err.code();
            let back = Refusal::of(&err).unwrap().into_error();
            assert_eq!(back.code(), code, "{err}");
            assert!(back.is_user_facing(), "{err}");
        }
        let unknown = Refusal {
            error: "something_new".into(),
            ..Refusal::default()
        }
        .into_error();
        assert!(!unknown.is_user_facing());
    }
}
