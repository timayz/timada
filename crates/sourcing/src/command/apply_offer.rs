use evento::Executor;
use timada_core::Money;
use timada_tax::{ExchangeRates, PinnedRate, RateError};

use crate::{
    connector::SupplierOffer,
    error::SourcingError,
    price::{Quote, ReviewReason, Verdict, quote},
    rule::resolve_rule,
};

/// What one supplier offer did to the shop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// What the rule made of the cost, and whether the shop moved its own
    /// price or has to ask.
    pub verdict: Verdict,
    /// The prices set in the shop's other currencies, if the base one moved
    /// and they did not follow. They are decisions, never conversions, so
    /// nothing here changes them — an operator is told instead.
    pub stale_currencies: Vec<String>,
}

impl<E: Executor> super::Command<'_, E> {
    /// Reads one supplier offer against the product's rule: converts the
    /// cost, prices it, and moves the selling price when the guardrails
    /// allow. The offer itself is recorded in `sourcing_offer` whatever the
    /// verdict — a supplier's word is operational data, not a fact worth an
    /// event.
    ///
    /// The caller passes the rates, so a worker asking about two hundred
    /// products shares one source and one cache.
    pub async fn apply_offer(
        &self,
        product_id: &str,
        offer: &SupplierOffer,
        rates: &dyn ExchangeRates,
        base_currency: &str,
        at: u64,
    ) -> Result<Applied, SourcingError> {
        let sourced = self
            .load_sourced_product(product_id)
            .await?
            .filter(|sourced| sourced.active)
            .ok_or(SourcingError::NotSourced)?;
        let supplier = self.require_supplier(&sourced.supplier_id).await?;
        if supplier.suspended {
            return Err(SourcingError::SupplierSuspended);
        }
        let rule = resolve_rule(&self.db, &supplier.id, product_id).await?;

        // What the shop pays for one unit, before any conversion.
        let mut landed = offer.cost.clone();
        if rule.shipping_included {
            landed = landed.checked_add(&offer.shipping)?;
        }

        let price =
            timada_pricing::load_product_price(self.executor, timada_pricing::price_id(product_id))
                .await?;
        // Without a price there is no currency to quote in and no VAT rate
        // to quote at: the listed currency stands in so the operator still
        // sees what the item costs.
        let selling_currency = price
            .as_ref()
            .filter(|price| !price.withdrawn)
            .map(|price| price.listed_currency().to_owned())
            .unwrap_or_else(|| base_currency.to_owned());

        let converted = convert(&landed, &selling_currency, base_currency, rates, at).await;
        let (landed, rate) = match converted {
            Ok(converted) => converted,
            Err(reason) => {
                self.record_offer(product_id, &supplier.id, offer, None, None)
                    .await?;
                return Ok(Applied {
                    verdict: Verdict::Review {
                        quote: unpriced_quote(&landed),
                        reason,
                    },
                    stale_currencies: Vec::new(),
                });
            }
        };
        self.record_offer(
            product_id,
            &supplier.id,
            offer,
            Some(&landed),
            rate.as_ref(),
        )
        .await?;

        // A locked price is not the sync's to move, nor to ask about.
        if sourced.locked {
            return Ok(Applied {
                verdict: Verdict::Unchanged,
                stale_currencies: Vec::new(),
            });
        }

        let Some(price) = price.filter(|price| !price.withdrawn) else {
            return Ok(Applied {
                verdict: Verdict::Review {
                    quote: unpriced_quote(&landed),
                    reason: ReviewReason::NoListedPrice,
                },
                stale_currencies: Vec::new(),
            });
        };
        let quoted = quote(&landed, price.vat_rate_bp, &price.eco_participation, &rule)?;
        let verdict = crate::price::verdict(Some(&price.price_incl_tax), quoted, &rule);

        let mut stale_currencies = Vec::new();
        if let Verdict::Apply(quoted) = &verdict {
            timada_pricing::Command(self.executor)
                .change_price(
                    timada_pricing::price_id(product_id),
                    quoted.price_incl_tax.clone(),
                )
                .await?;
            tracing::info!(
                %product_id,
                price = %quoted.price_incl_tax.minor,
                margin_bp = quoted.margin_bp,
                "selling price followed the supplier's cost"
            );
            stale_currencies = price
                .currency_prices
                .iter()
                .map(|money| money.currency.clone())
                .collect();
        }

        Ok(Applied {
            verdict,
            stale_currencies,
        })
    }

    async fn record_offer(
        &self,
        product_id: &str,
        supplier_id: &str,
        offer: &SupplierOffer,
        landed: Option<&Money>,
        rate: Option<&PinnedRate>,
    ) -> Result<(), SourcingError> {
        sqlx::query(
            "INSERT INTO sourcing_offer
                (product_id, supplier_id, cost_minor, cost_currency, shipping_minor,
                 available, title, url, landed_minor, landed_currency,
                 rate_micros, rate_source, rate_as_of, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (product_id) DO UPDATE SET
                supplier_id = excluded.supplier_id,
                cost_minor = excluded.cost_minor,
                cost_currency = excluded.cost_currency,
                shipping_minor = excluded.shipping_minor,
                available = excluded.available,
                title = excluded.title,
                url = excluded.url,
                landed_minor = excluded.landed_minor,
                landed_currency = excluded.landed_currency,
                rate_micros = excluded.rate_micros,
                rate_source = excluded.rate_source,
                rate_as_of = excluded.rate_as_of,
                fetched_at = excluded.fetched_at",
        )
        .bind(product_id)
        .bind(supplier_id)
        .bind(offer.cost.minor)
        .bind(&offer.cost.currency)
        .bind(offer.shipping.minor)
        .bind(offer.available)
        .bind(offer.title.as_deref())
        .bind(offer.url.as_deref())
        .bind(landed.map(|money| money.minor))
        .bind(landed.map(|money| money.currency.clone()))
        .bind(rate.map(|rate| rate.per_base_micros as i64))
        .bind(rate.map(|rate| rate.source.clone()))
        .bind(rate.map(|rate| rate.as_of as i64))
        .bind(timada_core::time::now_unix_secs()? as i64)
        .execute(&self.db)
        .await?;
        Ok(())
    }
}

/// A cost in the currency the product is sold in, and the rate it was read
/// at. Two legs are refused rather than chained: a rate arrived at through
/// a third currency is wrong by a fraction of a percent every time, and a
/// price is not the place to be casually wrong.
async fn convert(
    landed: &Money,
    selling: &str,
    base: &str,
    rates: &dyn ExchangeRates,
    at: u64,
) -> Result<(Money, Option<PinnedRate>), ReviewReason> {
    if landed.currency == selling {
        return Ok((landed.clone(), None));
    }
    let unavailable = |err: RateError| {
        tracing::warn!(%err, "no rate to price a supplier's cost with");
        ReviewReason::NoRate
    };
    if selling == base {
        let rate = rates
            .rate(base, &landed.currency, at)
            .await
            .map_err(unavailable)?;
        let converted = rate.to_base(landed).map_err(unavailable)?;
        return Ok((converted, Some(rate)));
    }
    if landed.currency == base {
        let rate = rates.rate(base, selling, at).await.map_err(unavailable)?;
        let converted = rate.from_base(landed).map_err(unavailable)?;
        return Ok((converted, Some(rate)));
    }
    tracing::warn!(
        cost = %landed.currency,
        %selling,
        %base,
        "a cost and a price in two currencies, neither of them the books': no single rate"
    );
    Err(ReviewReason::NoRate)
}

/// What there is to say about a cost that could not become a price.
fn unpriced_quote(landed: &Money) -> Quote {
    Quote {
        price_incl_tax: Money::zero(&landed.currency),
        price_excl_tax: Money::zero(&landed.currency),
        margin_bp: 0,
        landed: landed.clone(),
    }
}
