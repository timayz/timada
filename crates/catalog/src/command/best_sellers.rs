//! The shop's best sellers: the ranking the host works out from the sales
//! and a listing sorts by.

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::{BestSellers, Ranked};
use crate::error::CatalogError;
use crate::value_object::{MAX_BEST_SELLERS, best_sellers_id};

/// The ranking as one month's stream last gave it.
#[evento::projection(id = id, name = "timada-catalog/BestSellersState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct BestSellersState {
    pub id: String,
    /// The best seller first.
    pub product_ids: Vec<String>,
}

pub fn best_sellers_projection<E: Executor>() -> Projection<E, BestSellersState> {
    Projection::new::<BestSellers>()
        .handler(on_ranked())
        .strict()
}

#[evento::handler]
async fn on_ranked(event: Event<Ranked>, state: &mut BestSellersState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.product_ids = event.data.product_ids.to_owned();
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records the shop's best sellers as of `now` (unix seconds), the best
    /// first: the first [`MAX_BEST_SELLERS`] of `product_ids`, each once.
    /// Written on the stream of the month `now` falls in; the ranking that
    /// stream already gives is a no-op, and so is an empty one on a month
    /// that has none, unless the month before ended on one. Returns whether
    /// it was written.
    pub async fn rank_best_sellers(
        &self,
        actor: &str,
        product_ids: &[String],
        now: u64,
        routing_key: Option<String>,
    ) -> Result<bool, CatalogError> {
        let mut ranked: Vec<String> = Vec::with_capacity(product_ids.len().min(MAX_BEST_SELLERS));
        for id in product_ids {
            if ranked.len() == MAX_BEST_SELLERS {
                break;
            }
            if !ranked.contains(id) {
                ranked.push(id.clone());
            }
        }
        let id = best_sellers_id(now);
        let state = self.load_best_sellers(&id).await?;
        let mut write = match &state {
            Some(state) if state.product_ids == ranked => return Ok(false),
            Some(state) => state.write()?,
            // A month opening on no sale at all: said once, when the month
            // before ended on a ranking, so nothing stays ranked for ever.
            None if ranked.is_empty() && !self.ranked_the_month_before(now).await? => {
                return Ok(false);
            }
            None => {
                let mut write = evento::append(id);
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&Ranked {
                product_ids: ranked,
                ranked_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(true)
    }
}

impl<E: Executor> super::Command<E> {
    /// Whether the month before the one `now` falls in ended on a ranking
    /// that names something.
    async fn ranked_the_month_before(&self, now: u64) -> anyhow::Result<bool> {
        let (_, _, day) = timada_core::civil::civil_date(now);
        let before = now.saturating_sub(u64::from(day) * 86_400);
        Ok(self
            .load_best_sellers(&best_sellers_id(before))
            .await?
            .is_some_and(|state| !state.product_ids.is_empty()))
    }

    /// What the stream `id` ([`best_sellers_id`]) ranks, if it was ever
    /// written.
    pub async fn load_best_sellers(&self, id: &str) -> anyhow::Result<Option<BestSellersState>> {
        best_sellers_projection()
            .load(id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
