//! What a sale leaves: the price a shopper pays, less its VAT, what the
//! shop pays for the goods, for sending them, at the border, to its payment
//! provider and for its fixed costs. Pure arithmetic: the catalog knows the
//! costs, whoever asks gives the VAT rate (the catalog knows no tax law).

use std::collections::BTreeMap;
use std::fmt;

/// What the shop pays for one of a thing it sells, in the thing's currency.
/// Nothing a shopper is ever shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnitCosts {
    /// What its supplier charges for one; `None` when the shop has not
    /// said, and nothing is worked out from it then.
    pub cost_cents: Option<u64>,
    /// What sending it costs, when it is not the shop's usual.
    pub shipping_cents: Option<u64>,
    /// What the border charges for it, when it is not the shop's usual.
    pub import_cents: Option<u64>,
}

/// What the shop pays for the things one product sells, in the product's
/// currency: what its cost stream says
/// ([`ProductCost`](crate::aggregator::ProductCost)).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProductCosts {
    /// For one of a product sold as is.
    pub cost_cents: Option<u64>,
    /// For one of each combination the shop said the cost of, by variant
    /// id. One that is no longer a variant may linger: nothing asks for it.
    pub variants: BTreeMap<String, u64>,
    /// What sending one costs, when it is not the shop's usual.
    pub own_shipping_cents: Option<u64>,
    /// What the border charges for one, when it is not the shop's usual.
    pub own_import_cents: Option<u64>,
}

impl ProductCosts {
    /// What the shop pays for one of the product (`variant_id` `None`) or
    /// of one of its variants, when it said.
    pub fn cost_of(&self, variant_id: Option<&str>) -> Option<u64> {
        match variant_id {
            Some(variant_id) => self.variants.get(variant_id).copied(),
            None => self.cost_cents,
        }
    }

    /// Everything one of the product (`variant_id` `None`) or of one of its
    /// variants costs the shop.
    pub fn unit_costs(&self, variant_id: Option<&str>) -> UnitCosts {
        UnitCosts {
            cost_cents: self.cost_of(variant_id),
            shipping_cents: self.own_shipping_cents,
            import_cents: self.own_import_cents,
        }
    }

    /// Whether the shop said nothing of it.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What a sale costs the shop beyond the goods, in `currency`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShopCosts {
    pub currency: String,
    /// What sending a parcel costs the shop.
    pub shipping_cents: u64,
    /// What the border charges for one kind of item in a parcel.
    pub import_cents: u64,
    /// The share of the shop's fixed costs a sale carries.
    pub fixed_cents: u64,
    /// What the payment provider keeps of the amount paid, in basis points.
    pub payment_fee_bps: u32,
    /// And for each payment whatever its amount.
    pub payment_fee_cents: u64,
}

impl ShopCosts {
    /// What the payment provider keeps of a payment of `amount_cents`,
    /// rounded to the nearest cent.
    pub fn payment_fee(&self, amount_cents: u64) -> u64 {
        let share = (u128::from(amount_cents) * u128::from(self.payment_fee_bps) + 5_000) / 10_000;
        share as u64 + self.payment_fee_cents
    }
}

/// The largest share a payment provider may be said to keep: all of it.
pub const MAX_FEE_BPS: u32 = 10_000;

/// The VAT in a price that includes it at `vat_bps`: what is left once the
/// price without VAT is rounded to the nearest cent, halves up (as an
/// invoice works it out).
pub fn vat_in(price_cents: u64, vat_bps: u32) -> u64 {
    let divisor = 10_000 + u128::from(vat_bps);
    let excl = (u128::from(price_cents) * 10_000 + divisor / 2) / divisor;
    price_cents - excl as u64
}

/// What selling one of a thing on its own leaves, line by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Margin {
    /// What the shopper pays, VAT included.
    pub price_cents: u64,
    pub vat_cents: u64,
    pub cost_cents: u64,
    pub shipping_cents: u64,
    pub import_cents: u64,
    pub fixed_cents: u64,
    pub payment_fee_cents: u64,
}

impl Margin {
    /// The margin of one of a thing sold alone at `price_cents`, VAT
    /// included at `vat_bps`, in `currency`. `None` when its cost is not
    /// known, or when the shop's costs are in another currency: nothing
    /// true can be said then.
    pub fn of(
        price_cents: u64,
        currency: &str,
        vat_bps: u32,
        unit: UnitCosts,
        shop: Option<&ShopCosts>,
    ) -> Option<Self> {
        let cost_cents = unit.cost_cents?;
        if shop.is_some_and(|shop| shop.currency != currency) {
            return None;
        }
        Some(Self {
            price_cents,
            vat_cents: vat_in(price_cents, vat_bps),
            cost_cents,
            shipping_cents: unit
                .shipping_cents
                .unwrap_or(shop.map_or(0, |shop| shop.shipping_cents)),
            import_cents: unit
                .import_cents
                .unwrap_or(shop.map_or(0, |shop| shop.import_cents)),
            fixed_cents: shop.map_or(0, |shop| shop.fixed_cents),
            payment_fee_cents: shop.map_or(0, |shop| shop.payment_fee(price_cents)),
        })
    }

    /// Everything the sale costs, VAT included.
    pub fn costs_cents(&self) -> u64 {
        self.vat_cents
            + self.cost_cents
            + self.shipping_cents
            + self.import_cents
            + self.fixed_cents
            + self.payment_fee_cents
    }

    /// What is left of the price; negative when the sale loses money.
    pub fn margin_cents(&self) -> i64 {
        self.price_cents as i64 - self.costs_cents() as i64
    }

    /// The margin as a share of the price, in basis points; `None` for a
    /// thing given away.
    pub fn rate_bps(&self) -> Option<i64> {
        (self.price_cents > 0).then(|| self.margin_cents() * 10_000 / self.price_cents as i64)
    }

    /// The return on ad spend at which an ad for it pays for itself and no
    /// more, in hundredths (`250` is 2.5: each euro of ads must bring 2.50
    /// of sales): the price over the margin. `None` when the sale leaves
    /// nothing, as no ad pays for itself then.
    pub fn break_even_roas(&self) -> Option<u64> {
        let margin = u64::try_from(self.margin_cents()).ok().filter(|m| *m > 0)?;
        Some((self.price_cents * 100).div_ceil(margin))
    }
}

/// How a margin is graded, for campaigns to group products by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarginTier {
    /// The sale leaves nothing, or loses money.
    Loss,
    Low,
    Medium,
    High,
}

impl MarginTier {
    /// What a feed's label says.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loss => "loss",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl fmt::Display for MarginTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// From which share of the price a margin is `medium` and `high`, and the
/// feed label that carries the tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarginTiers {
    pub medium_from_bps: u32,
    pub high_from_bps: u32,
    /// Which of the feed's labels carries the tier, from 0; `None` leaves
    /// the labels to the merchant.
    pub feed_label: Option<u8>,
}

impl Default for MarginTiers {
    /// A fifth of the price is a medium margin, two fifths a high one.
    fn default() -> Self {
        Self {
            medium_from_bps: 2_000,
            high_from_bps: 4_000,
            feed_label: None,
        }
    }
}

impl MarginTiers {
    pub fn tier(&self, margin: &Margin) -> MarginTier {
        match margin.rate_bps() {
            Some(rate) if margin.margin_cents() > 0 => {
                if rate >= i64::from(self.high_from_bps) {
                    MarginTier::High
                } else if rate >= i64::from(self.medium_from_bps) {
                    MarginTier::Medium
                } else {
                    MarginTier::Low
                }
            }
            _ => MarginTier::Loss,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shop() -> ShopCosts {
        ShopCosts {
            currency: "EUR".into(),
            shipping_cents: 400,
            import_cents: 500,
            fixed_cents: 100,
            payment_fee_bps: 150,
            payment_fee_cents: 25,
        }
    }

    fn cost(cost_cents: u64) -> UnitCosts {
        UnitCosts {
            cost_cents: Some(cost_cents),
            ..UnitCosts::default()
        }
    }

    #[test]
    fn the_vat_in_a_price_is_what_an_invoice_takes_out_of_it() {
        assert_eq!(vat_in(1200, 2_000), 200);
        assert_eq!(vat_in(1000, 550), 52);
        assert_eq!(vat_in(999, 2_000), 166);
        assert_eq!(vat_in(1200, 0), 0);
    }

    #[test]
    fn a_margin_is_the_price_less_everything_the_sale_costs() {
        let margin = Margin::of(6_000, "EUR", 2_000, cost(1_500), Some(&shop())).unwrap();
        assert_eq!(margin.vat_cents, 1_000);
        // 1.5 % of 60.00 and 0.25.
        assert_eq!(margin.payment_fee_cents, 115);
        assert_eq!(margin.costs_cents(), 1_000 + 1_500 + 400 + 500 + 100 + 115);
        assert_eq!(margin.margin_cents(), 2_385);
        assert_eq!(margin.rate_bps(), Some(3_975));
        // 60.00 / 23.85, rounded up: an ad at 2.51 loses nothing.
        assert_eq!(margin.break_even_roas(), Some(252));
    }

    #[test]
    fn a_product_s_own_costs_stand_in_for_the_shop_s() {
        let unit = UnitCosts {
            cost_cents: Some(1_500),
            shipping_cents: Some(0),
            import_cents: Some(0),
        };
        let margin = Margin::of(6_000, "EUR", 2_000, unit, Some(&shop())).unwrap();
        assert_eq!(margin.shipping_cents, 0);
        assert_eq!(margin.import_cents, 0);
        assert_eq!(margin.margin_cents(), 3_285);
    }

    #[test]
    fn without_a_cost_or_in_another_currency_nothing_is_said() {
        assert_eq!(
            Margin::of(6_000, "EUR", 2_000, UnitCosts::default(), Some(&shop())),
            None
        );
        assert_eq!(
            Margin::of(6_000, "USD", 2_000, cost(1_500), Some(&shop())),
            None
        );
        // A shop that told no cost of its own counts none.
        let margin = Margin::of(6_000, "USD", 0, cost(1_500), None).unwrap();
        assert_eq!(margin.margin_cents(), 4_500);
    }

    #[test]
    fn a_sale_that_leaves_nothing_has_no_break_even() {
        let margin = Margin::of(1_000, "EUR", 2_000, cost(900), Some(&shop())).unwrap();
        assert!(margin.margin_cents() < 0);
        assert_eq!(margin.break_even_roas(), None);
        assert_eq!(MarginTiers::default().tier(&margin), MarginTier::Loss);
    }

    #[test]
    fn a_margin_is_graded_by_its_share_of_the_price() {
        let tiers = MarginTiers::default();
        let at = |cost_cents| {
            let margin = Margin::of(10_000, "EUR", 0, cost(cost_cents), None).unwrap();
            tiers.tier(&margin)
        };
        assert_eq!(at(10_000), MarginTier::Loss);
        assert_eq!(at(8_001), MarginTier::Low);
        assert_eq!(at(8_000), MarginTier::Medium);
        assert_eq!(at(6_001), MarginTier::Medium);
        assert_eq!(at(6_000), MarginTier::High);
    }
}
