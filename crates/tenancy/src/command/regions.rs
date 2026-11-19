use std::collections::BTreeSet;

use evento::ProjectionAggregate;
use evento::{Executor, Projection, WriteError, metadata::Event};

use crate::aggregator::regions::{Opened, Regions};
use crate::value_object::regions_id;

use super::Command;

/// The key the list is written under by every node, whatever its own
/// region. Two nodes that create a stream at once under two keys are not
/// set against each other by the store, which goes by the key and the
/// stream together: both writes are taken, neither can be applied, and the
/// stream is read by no one from then on. The first nodes of a cluster say
/// their regions at the same moment.
const REGIONS_KEY: &str = "regions";

/// How many times a region is written again after another node's region
/// got onto the list first.
const OPEN_ATTEMPTS: usize = 3;

#[evento::projection(id = id, name = "timada-tenancy/RegionsState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct RegionsState {
    pub id: String,
    pub regions: BTreeSet<String>,
}

pub fn regions_projection<E: Executor>() -> Projection<E, RegionsState> {
    Projection::new::<Regions>()
        .handler(on_region_opened())
        .strict()
}

#[evento::handler]
async fn on_region_opened(event: Event<Opened>, list: &mut RegionsState) -> anyhow::Result<()> {
    list.id = event.aggregate_id.to_owned();
    list.regions.insert(event.data.region.to_owned());
    Ok(())
}

impl<E: Executor> Command<E> {
    /// The regions the nodes ever wrote under.
    pub async fn load_regions(&self) -> anyhow::Result<BTreeSet<String>> {
        Ok(self
            .load_regions_state()
            .await?
            .map(|list| list.regions)
            .unwrap_or_default())
    }

    async fn load_regions_state(&self) -> anyhow::Result<Option<RegionsState>> {
        regions_projection()
            .load(regions_id())
            .execute(&self.0)
            .await
    }

    /// Puts `region` on the list unless it is there. The nodes of a cluster
    /// say theirs as they start, side by side: a write that lost to
    /// another's is made again on the list as it then stands.
    pub async fn open_region(&self, region: &str) -> anyhow::Result<()> {
        let opened = Opened {
            region: region.to_owned(),
        };
        for _ in 0..OPEN_ATTEMPTS {
            let written = match self.load_regions_state().await? {
                Some(list) if list.regions.contains(region) => return Ok(()),
                Some(list) => list.write()?.event(&opened).commit(&self.0).await,
                None => {
                    evento::append(regions_id())
                        .original_version(0)
                        .routing_key(REGIONS_KEY)
                        .event(&opened)
                        .commit(&self.0)
                        .await
                }
            };
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        anyhow::bail!("the list of regions kept changing: {region} is not on it yet")
    }
}
