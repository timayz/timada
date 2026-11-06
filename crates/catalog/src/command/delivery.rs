//! How long delivery takes: per product, else the shop's default.

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::{DefaultDeliveryDaysSet, DeliveryDaysSet};
use crate::error::CatalogError;
use crate::value_object::{DeliveryDays, delivery_default_id};

#[evento::projection(id = id, name = "timada-catalog/DeliveryDefaultState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct DeliveryDefaultState {
    pub id: String,
    pub days: Option<DeliveryDays>,
}

pub fn delivery_default_projection<E: Executor>() -> Projection<E, DeliveryDefaultState> {
    Projection::new::<crate::aggregator::DeliveryDefault>()
        .handler(on_default_delivery_days_set())
        .strict()
}

#[evento::handler]
async fn on_default_delivery_days_set(
    event: Event<DefaultDeliveryDaysSet>,
    state: &mut DeliveryDefaultState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.days = event.data.days;
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets how long delivery of the product takes; `None` falls back to the
    /// shop's default. The same days are a no-op.
    pub async fn set_product_delivery_days(
        &self,
        actor: &str,
        product_id: &str,
        days: Option<DeliveryDays>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if let Some(days) = days {
            DeliveryDays::new(days.min, days.max)?;
        }
        let product = self.require_editable(product_id).await?;
        if product.delivery_days == days {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&DeliveryDaysSet {
                days,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets the delivery time of every product that gives none; `None`
    /// announces none. The same days are a no-op.
    pub async fn set_default_delivery_days(
        &self,
        actor: &str,
        days: Option<DeliveryDays>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if let Some(days) = days {
            DeliveryDays::new(days.min, days.max)?;
        }
        let state = self.load_delivery_default_state().await?;
        if state.as_ref().and_then(|state| state.days) == days {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(delivery_default_id());
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&DefaultDeliveryDaysSet {
                days,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    /// The shop's default delivery time, if it gives one.
    pub async fn load_default_delivery_days(&self) -> anyhow::Result<Option<DeliveryDays>> {
        Ok(self
            .load_delivery_default_state()
            .await?
            .and_then(|state| state.days))
    }

    async fn load_delivery_default_state(&self) -> anyhow::Result<Option<DeliveryDefaultState>> {
        delivery_default_projection()
            .load(delivery_default_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
