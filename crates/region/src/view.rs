//! The write-side view of a region, replayed from its events.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::{Currency, Executor};

use crate::aggregate::{Region, RegionCountry, RegionCreated, RegionUpdated};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct RegionView {
    pub id: String,
    pub name: String,
    pub currency: Currency,
    pub countries: Vec<RegionCountry>,
}

impl ProjectionAggregate for RegionView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn apply_created(event: Event<RegionCreated>, view: &mut RegionView) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.name = event.data.name.clone();
    view.currency = event.data.currency;
    view.countries = event.data.countries.clone();
    Ok(())
}

#[evento::handler]
async fn apply_updated(event: Event<RegionUpdated>, view: &mut RegionView) -> anyhow::Result<()> {
    view.name = event.data.name.clone();
    view.countries = event.data.countries.clone();
    Ok(())
}

/// Replay one region. `None` means no such aggregate.
pub async fn load_region(
    executor: &Executor,
    region_id: &str,
) -> anyhow::Result<Option<RegionView>> {
    Projection::<_, RegionView>::new::<Region>()
        .handler(apply_created())
        .handler(apply_updated())
        .strict()
        .load(region_id)
        .execute(executor)
        .await
}
