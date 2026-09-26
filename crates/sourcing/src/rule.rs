//! How a supplier's cost becomes a selling price.
//!
//! The rule is **configuration, not a fact**: it is tuned every time a margin
//! disappoints, and a shape frozen in `events.lock` could never be tuned
//! again. It lives in `sourcing_rule`, where a migration can change it, the
//! way [`timada_returns::ReturnPolicy`]-style values live outside the event
//! store. What the rule *produces* — a new price — is already an event, in
//! `timada-pricing`.
//!
//! [`timada_returns::ReturnPolicy`]: https://docs.rs/timada-returns

use sqlx::SqlitePool;
use timada_core::Money;

/// Who a rule applies to. The narrowest one wins: a product's own, else its
/// supplier's, else the shop's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleScope {
    Default,
    Supplier(String),
    Product(String),
}

impl RuleScope {
    /// The primary key the scope is stored under.
    fn key(&self) -> String {
        match self {
            Self::Default => "default".to_owned(),
            Self::Supplier(id) => format!("supplier:{id}"),
            Self::Product(id) => format!("product:{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingRule {
    /// Markup on the landed cost, in basis points: 6000 = +60 %. Markup on
    /// cost, not margin on price — it is what an operator types; what the
    /// price actually comes out at is reported back next to it.
    pub markup_bp: u16,
    /// Under this realised markup the price is never applied by itself.
    ///
    /// It bites rarely by design: rounding is always upwards, so a price
    /// normally comes out at `markup_bp` or a little over. What it catches
    /// is a `markup_bp` somebody set below the minimum they meant to keep,
    /// and an éco-participation absorbed out of the margin
    /// (`eco_on_top = false`) on a product dear enough for it to matter.
    pub min_margin_bp: u16,
    /// Round *up* to a multiple of this many minor units …
    pub round_step_minor: i64,
    /// … and then land on these. 90 gives « …,90 »; a step of 1 ending on 0
    /// is no rounding at all.
    pub round_ends_minor: i64,
    /// A move of at most this, relative to the price now, goes through by
    /// itself …
    pub auto_move_bp: u16,
    /// … and of at most this, absolute. Both must hold: the relative test
    /// alone lets a cheap item swing wildly, the absolute one alone lets an
    /// expensive one creep.
    pub auto_move_cap: Money,
    /// Whether what the supplier charges to ship counts as cost.
    pub shipping_included: bool,
    /// Whether the éco-participation is added on top of the margin.
    pub eco_on_top: bool,
    /// Units of the supplier's level never published, so the shop is never
    /// the one selling its last one.
    pub safety_stock: u32,
}

impl Default for PricingRule {
    fn default() -> Self {
        Self {
            markup_bp: 6_000,
            min_margin_bp: 2_000,
            round_step_minor: 100,
            round_ends_minor: 90,
            auto_move_bp: 500,
            auto_move_cap: Money::eur(1_000),
            shipping_included: true,
            eco_on_top: true,
            safety_stock: 1,
        }
    }
}

impl PricingRule {
    /// The price, rounded the way the rule asks: always upwards, so rounding
    /// can only improve the margin, never eat it.
    pub fn round(&self, price: &Money) -> Money {
        let step = self.round_step_minor.max(1);
        let ends = self.round_ends_minor.rem_euclid(step);
        let from = price.minor - ends;
        let steps = from.div_euclid(step) + i64::from(from.rem_euclid(step) != 0);
        Money::new(steps * step + ends, &price.currency)
    }
}

/// The rule for this product at this supplier: its own, else the supplier's,
/// else the shop's, else the built-in one.
pub async fn resolve_rule(
    db: &SqlitePool,
    supplier_id: &str,
    product_id: &str,
) -> sqlx::Result<PricingRule> {
    for scope in [
        RuleScope::Product(product_id.to_owned()),
        RuleScope::Supplier(supplier_id.to_owned()),
        RuleScope::Default,
    ] {
        if let Some(rule) = load_rule(db, &scope).await? {
            return Ok(rule);
        }
    }
    Ok(PricingRule::default())
}

/// The rule stored for exactly this scope, without falling back.
pub async fn load_rule(db: &SqlitePool, scope: &RuleScope) -> sqlx::Result<Option<PricingRule>> {
    let row: Option<RuleRow> = sqlx::query_as(
        "SELECT markup_bp, min_margin_bp, round_step_minor, round_ends_minor,
                auto_move_bp, auto_move_cap_minor, auto_move_cap_currency,
                shipping_included, eco_on_top, safety_stock
         FROM sourcing_rule WHERE scope = ?",
    )
    .bind(scope.key())
    .fetch_optional(db)
    .await?;
    Ok(row.map(Into::into))
}

pub async fn save_rule(db: &SqlitePool, scope: &RuleScope, rule: &PricingRule) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO sourcing_rule
            (scope, markup_bp, min_margin_bp, round_step_minor, round_ends_minor,
             auto_move_bp, auto_move_cap_minor, auto_move_cap_currency,
             shipping_included, eco_on_top, safety_stock)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (scope) DO UPDATE SET
            markup_bp = excluded.markup_bp,
            min_margin_bp = excluded.min_margin_bp,
            round_step_minor = excluded.round_step_minor,
            round_ends_minor = excluded.round_ends_minor,
            auto_move_bp = excluded.auto_move_bp,
            auto_move_cap_minor = excluded.auto_move_cap_minor,
            auto_move_cap_currency = excluded.auto_move_cap_currency,
            shipping_included = excluded.shipping_included,
            eco_on_top = excluded.eco_on_top,
            safety_stock = excluded.safety_stock",
    )
    .bind(scope.key())
    .bind(rule.markup_bp)
    .bind(rule.min_margin_bp)
    .bind(rule.round_step_minor)
    .bind(rule.round_ends_minor)
    .bind(rule.auto_move_bp)
    .bind(rule.auto_move_cap.minor)
    .bind(&rule.auto_move_cap.currency)
    .bind(rule.shipping_included)
    .bind(rule.eco_on_top)
    .bind(rule.safety_stock)
    .execute(db)
    .await?;
    Ok(())
}

/// Back to the level above: the shop's rule for a supplier, the supplier's
/// for a product.
pub async fn clear_rule(db: &SqlitePool, scope: &RuleScope) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sourcing_rule WHERE scope = ?")
        .bind(scope.key())
        .execute(db)
        .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct RuleRow {
    markup_bp: i64,
    min_margin_bp: i64,
    round_step_minor: i64,
    round_ends_minor: i64,
    auto_move_bp: i64,
    auto_move_cap_minor: i64,
    auto_move_cap_currency: String,
    shipping_included: bool,
    eco_on_top: bool,
    safety_stock: i64,
}

impl From<RuleRow> for PricingRule {
    fn from(row: RuleRow) -> Self {
        Self {
            markup_bp: row.markup_bp.clamp(0, i64::from(u16::MAX)) as u16,
            min_margin_bp: row.min_margin_bp.clamp(0, i64::from(u16::MAX)) as u16,
            round_step_minor: row.round_step_minor,
            round_ends_minor: row.round_ends_minor,
            auto_move_bp: row.auto_move_bp.clamp(0, i64::from(u16::MAX)) as u16,
            auto_move_cap: Money::new(row.auto_move_cap_minor, row.auto_move_cap_currency),
            shipping_included: row.shipping_included,
            eco_on_top: row.eco_on_top,
            safety_stock: row.safety_stock.clamp(0, i64::from(u32::MAX)) as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_up_onto_the_charm_ending() {
        let rule = PricingRule::default();
        assert_eq!(rule.round(&Money::eur(6_458)), Money::eur(6_490));
        // Already there: left alone, not pushed to the next step.
        assert_eq!(rule.round(&Money::eur(6_490)), Money::eur(6_490));
        // Just past it: the next rung of the ladder.
        assert_eq!(rule.round(&Money::eur(6_491)), Money::eur(6_590));
        // Below the first ending.
        assert_eq!(rule.round(&Money::eur(12)), Money::eur(90));
        let exact = PricingRule {
            round_step_minor: 1,
            round_ends_minor: 0,
            ..PricingRule::default()
        };
        assert_eq!(exact.round(&Money::eur(6_458)), Money::eur(6_458));
    }
}
