//! The options a shop's listings are narrowed by.

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::{FiltersSet, ListingFilters};
use crate::error::CatalogError;
use crate::value_object::{MAX_LISTING_FILTERS, listing_filters_id, normalize_option_name};

#[evento::projection(id = id, name = "timada-catalog/ListingFiltersState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ListingFiltersState {
    pub id: String,
    /// The options' names, in the order the filters show them.
    pub names: Vec<String>,
}

pub fn listing_filters_projection<E: Executor>() -> Projection<E, ListingFiltersState> {
    Projection::new::<ListingFilters>()
        .handler(on_filters_set())
        .strict()
}

#[evento::handler]
async fn on_filters_set(
    event: Event<FiltersSet>,
    state: &mut ListingFiltersState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.names = event.data.names.to_owned();
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets the options the shop's listings are narrowed by, in the order
    /// given: each named as the products name it, an option or a fact of
    /// their tables, one named twice (case aside) counting once, [`MAX_LISTING_FILTERS`] at most. None offers
    /// no such filter. The same list is a no-op.
    pub async fn set_listing_filters(
        &self,
        actor: &str,
        names: &[String],
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let mut wanted: Vec<String> = Vec::new();
        for name in names.iter().filter(|name| !name.trim().is_empty()) {
            let name = normalize_option_name(name)?;
            if !wanted
                .iter()
                .any(|seen| seen.to_lowercase() == name.to_lowercase())
            {
                wanted.push(name);
            }
        }
        if wanted.len() > MAX_LISTING_FILTERS {
            return Err(CatalogError::TooManyListingFilters);
        }
        let state = self.load_listing_filters_state().await?;
        let mut write = match &state {
            Some(state) if state.names == wanted => return Ok(()),
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(listing_filters_id());
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&FiltersSet {
                names: wanted,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    /// The options the shop's listings are narrowed by; `None` while the
    /// shop never set them.
    pub async fn load_listing_filters(&self) -> anyhow::Result<Option<Vec<String>>> {
        Ok(self
            .load_listing_filters_state()
            .await?
            .map(|state| state.names))
    }

    async fn load_listing_filters_state(&self) -> anyhow::Result<Option<ListingFiltersState>> {
        listing_filters_projection()
            .load(listing_filters_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
