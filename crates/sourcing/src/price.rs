//! Cost in, selling price out — and the guardrails that decide whether the
//! shop may move the price by itself or has to ask.
//!
//! Everything here is a pure function of a cost, a VAT rate and a rule, so
//! the arithmetic is tested as arithmetic. Nothing in this module reads the
//! database or writes an event.

use timada_core::Money;

use crate::{error::SourcingError, rule::PricingRule};

/// What the rule makes of a cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    /// What the shopper would pay, rounded.
    pub price_incl_tax: Money,
    /// The same, without VAT and without the éco-participation: what the
    /// shop keeps for the goods.
    pub price_excl_tax: Money,
    /// What that comes to as a markup on the landed cost, in basis points.
    /// Negative means the shop would be selling at a loss.
    pub margin_bp: i32,
    /// One unit's cost, landed, in the currency it is sold in.
    pub landed: Money,
}

/// Why a price the rule computed is not the shop's to apply by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewReason {
    /// Further from the price now than the rule allows on its own.
    Jump,
    /// It would sell under the rule's minimum markup.
    Floor,
    /// The product has no price to change: sourcing never *lists* a price,
    /// because the VAT rate, the éco-participation and the instalment offer
    /// that come with one are the operator's to decide.
    NoListedPrice,
    /// The cost is in another currency and no rate could be had.
    NoRate,
    /// The base price moved, and the prices set in the shop's other
    /// currencies did not follow — those are decisions, never conversions.
    CurrencyPricesStale,
}

impl ReviewReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jump => "jump",
            Self::Floor => "floor",
            Self::NoListedPrice => "no-price",
            Self::NoRate => "no-rate",
            Self::CurrencyPricesStale => "currencies-stale",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "jump" => Some(Self::Jump),
            "floor" => Some(Self::Floor),
            "no-price" => Some(Self::NoListedPrice),
            "no-rate" => Some(Self::NoRate),
            "currencies-stale" => Some(Self::CurrencyPricesStale),
            _ => None,
        }
    }

    /// How the back office words it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Jump => "Écart trop important",
            Self::Floor => "Marge insuffisante",
            Self::NoListedPrice => "Produit sans prix",
            Self::NoRate => "Cours indisponible",
            Self::CurrencyPricesStale => "Prix en devise à revoir",
        }
    }
}

/// What to do with a quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Already that price. Nothing is written — `change_price` has no
    /// equality guard of its own, and a feed polled every few hours would
    /// otherwise append an event per product per pass, for ever.
    Unchanged,
    /// Inside the guardrails: the shop moves its own price.
    Apply(Quote),
    /// Outside them: an operator decides.
    Review { quote: Quote, reason: ReviewReason },
}

/// The selling price a landed cost earns under the rule.
///
/// `landed` and `eco` are in the currency the product is sold in; `eco` is
/// the éco-participation the product already carries, which is never marked
/// up — it is a contribution passed on, not goods.
pub fn quote(
    landed: &Money,
    vat_rate_bp: u16,
    eco: &Money,
    rule: &PricingRule,
) -> Result<Quote, SourcingError> {
    // Markup on cost, before tax. `markup_bp` is a u16, so this cannot ask
    // `percent_bp` for more than it can give.
    let excl_tax = landed.checked_add(&landed.percent_bp(rule.markup_bp))?;
    let with_vat = excl_tax.incl_tax(vat_rate_bp);
    let asked = if rule.eco_on_top {
        with_vat.checked_add(eco)?
    } else {
        with_vat
    };
    let price_incl_tax = rule.round(&asked);

    // What the shop is left with for the goods, whichever way the
    // éco-participation was handled: it owes it either way.
    let goods = price_incl_tax.checked_sub(eco)?;
    let price_excl_tax = goods.excl_tax(vat_rate_bp);
    Ok(Quote {
        margin_bp: markup_bp(landed, &price_excl_tax),
        price_incl_tax,
        price_excl_tax,
        landed: landed.clone(),
    })
}

/// What `price_excl_tax` is as a markup on `landed`, in basis points. A cost
/// of nothing has no markup to speak of, so anything sold above it counts as
/// the largest there is.
fn markup_bp(landed: &Money, price_excl_tax: &Money) -> i32 {
    if landed.minor <= 0 {
        return if price_excl_tax.minor > 0 {
            i32::MAX
        } else {
            0
        };
    }
    let over = i128::from(price_excl_tax.minor) - i128::from(landed.minor);
    let bp = over * 10_000 / i128::from(landed.minor);
    bp.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32
}

/// Whether the shop may move its price to the quote by itself.
///
/// `current` is the price now; `None` means the product has no price stream,
/// which sourcing will not open for it.
pub fn verdict(current: Option<&Money>, quote: Quote, rule: &PricingRule) -> Verdict {
    let Some(current) = current else {
        return Verdict::Review {
            quote,
            reason: ReviewReason::NoListedPrice,
        };
    };
    if current == &quote.price_incl_tax {
        return Verdict::Unchanged;
    }
    if quote.margin_bp < i32::from(rule.min_margin_bp) {
        return Verdict::Review {
            quote,
            reason: ReviewReason::Floor,
        };
    }
    if within_guardrail(current, &quote.price_incl_tax, rule) {
        Verdict::Apply(quote)
    } else {
        Verdict::Review {
            quote,
            reason: ReviewReason::Jump,
        }
    }
}

/// Both tests must hold, in both directions. The relative one alone lets a
/// cheap item swing wildly; the absolute one alone lets an expensive one
/// creep; and only testing a rise would let a feed that read `$3.45` as
/// `$0.345` cut every price by nine tenths unasked.
fn within_guardrail(current: &Money, proposed: &Money, rule: &PricingRule) -> bool {
    // A cap the price cannot be compared with is no cap at all: ask.
    if rule.auto_move_cap.currency != proposed.currency || current.currency != proposed.currency {
        return false;
    }
    let moved = (i128::from(proposed.minor) - i128::from(current.minor)).abs();
    if moved > i128::from(rule.auto_move_cap.minor.max(0)) {
        return false;
    }
    // Moving away from nothing is a move of no known size.
    if current.minor <= 0 {
        return false;
    }
    moved * 10_000 / i128::from(current.minor) <= i128::from(rule.auto_move_bp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> PricingRule {
        PricingRule {
            markup_bp: 6_000,
            min_margin_bp: 4_000,
            ..PricingRule::default()
        }
    }

    #[test]
    fn a_cost_earns_a_charm_price_and_reports_its_markup() -> Result<(), SourcingError> {
        // 33,64 € landed, +60 % → 53,82 € HT, +20 % VAT → 64,58 € TTC,
        // rounded up onto the charm ending → 64,90 €.
        let quoted = quote(&Money::eur(3_364), 2000, &Money::eur(0), &rule())?;
        assert_eq!(quoted.price_incl_tax, Money::eur(6_490));
        // Which is 54,08 € HT, so rounding bought a little more margin than
        // the 6000 bp asked for.
        assert_eq!(quoted.price_excl_tax, Money::eur(5_408));
        assert_eq!(quoted.margin_bp, 6_076);
        Ok(())
    }

    #[test]
    fn the_eco_participation_rides_on_top_and_is_never_marked_up() -> Result<(), SourcingError> {
        let eco = Money::eur(150);
        let on_top = quote(&Money::eur(3_364), 2000, &eco, &rule())?;
        // The same 64,58 € plus 1,50 €, rounded up from 66,08 €.
        assert_eq!(on_top.price_incl_tax, Money::eur(6_690));
        // The shop keeps the goods' share, not the contribution.
        assert_eq!(
            on_top.price_excl_tax,
            Money::eur(6_690 - 150).excl_tax(2000)
        );

        let absorbed = quote(
            &Money::eur(3_364),
            2000,
            &eco,
            &PricingRule {
                eco_on_top: false,
                ..rule()
            },
        )?;
        assert_eq!(absorbed.price_incl_tax, Money::eur(6_490));
        // Absorbing it comes straight out of the margin.
        assert!(absorbed.margin_bp < on_top.margin_bp);
        Ok(())
    }

    #[test]
    fn a_quote_is_applied_only_inside_both_guardrails() -> Result<(), SourcingError> {
        let rule = rule();
        let quoted = quote(&Money::eur(3_364), 2000, &Money::eur(0), &rule)?;

        // Already there.
        assert_eq!(
            verdict(Some(&Money::eur(6_490)), quoted.clone(), &rule),
            Verdict::Unchanged
        );
        // 64,90 from 63,90 is 1,00 € and 156 bp: through.
        assert!(matches!(
            verdict(Some(&Money::eur(6_390)), quoted.clone(), &rule),
            Verdict::Apply(_)
        ));
        // From 59,90 it is 5,00 € and 835 bp: too far relatively.
        assert!(matches!(
            verdict(Some(&Money::eur(5_990)), quoted.clone(), &rule),
            Verdict::Review {
                reason: ReviewReason::Jump,
                ..
            }
        ));
        // A fall is tested the same way — a misread feed cuts nothing.
        assert!(matches!(
            verdict(Some(&Money::eur(20_000)), quoted.clone(), &rule),
            Verdict::Review {
                reason: ReviewReason::Jump,
                ..
            }
        ));
        // Relatively small, absolutely large: still too far. 5,00 € is a
        // sixth of the way at 5000 bp, but it is over a 2,00 € cap.
        let dear = PricingRule {
            auto_move_bp: 5_000,
            auto_move_cap: Money::eur(200),
            ..rule.clone()
        };
        assert!(matches!(
            verdict(Some(&Money::eur(5_990)), quoted.clone(), &dear),
            Verdict::Review {
                reason: ReviewReason::Jump,
                ..
            }
        ));
        // A product with no price of its own is never given one here.
        assert!(matches!(
            verdict(None, quoted, &rule),
            Verdict::Review {
                reason: ReviewReason::NoListedPrice,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn selling_under_the_floor_is_always_asked_about() -> Result<(), SourcingError> {
        let rule = PricingRule {
            markup_bp: 500,
            min_margin_bp: 4_000,
            ..rule()
        };
        let quoted = quote(&Money::eur(3_364), 2000, &Money::eur(0), &rule)?;
        assert!(quoted.margin_bp < 4_000);
        // Even a move small enough to pass the guardrails.
        assert!(matches!(
            verdict(Some(&quoted.price_incl_tax.clone()), quoted.clone(), &rule),
            Verdict::Unchanged
        ));
        assert!(matches!(
            verdict(
                Some(&Money::new(quoted.price_incl_tax.minor - 10, "EUR")),
                quoted,
                &rule
            ),
            Verdict::Review {
                reason: ReviewReason::Floor,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn a_cost_of_nothing_is_all_markup() -> Result<(), SourcingError> {
        let quoted = quote(&Money::eur(0), 2000, &Money::eur(0), &rule())?;
        // Rounding still puts it on the charm ladder rather than at zero.
        assert_eq!(quoted.price_incl_tax, Money::eur(90));
        assert_eq!(quoted.margin_bp, i32::MAX);
        // Whatever is asked about it, it is never the floor: selling a free
        // sample for anything clears any minimum markup there is. (A ten
        // cent move on an eighty cent price is an eighth of it, so this one
        // is the guardrail's doing.)
        assert!(matches!(
            verdict(Some(&Money::eur(80)), quoted, &rule()),
            Verdict::Review {
                reason: ReviewReason::Jump,
                ..
            }
        ));
        Ok(())
    }
}
