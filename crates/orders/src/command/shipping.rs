use evento::{Executor, ProjectionAggregate};
use ulid::Ulid;

use crate::aggregator::{MethodAdded, MethodChanged, MethodRemoved};
use crate::error::OrdersError;
use crate::value_object::{
    MAX_SHIPPING_METHODS, NewShippingMethod, normalize_shipping_method, shipping_methods_id,
};

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// The ways the shop ships, in the order they were added; empty while
    /// shipping is included.
    pub async fn shipping_methods(&self) -> anyhow::Result<Vec<super::ShippingMethod>> {
        Ok(self
            .load_shipping_methods()
            .await?
            .map(|state| state.methods)
            .unwrap_or_default())
    }

    /// The merchant adds a way to ship; returns its id.
    pub async fn add_shipping_method(
        &self,
        actor: &str,
        method: NewShippingMethod,
        routing_key: Option<String>,
    ) -> Result<String, OrdersError> {
        let method = normalize_shipping_method(method)?;
        let state = self.load_shipping_methods().await?;
        if state
            .as_ref()
            .is_some_and(|state| state.methods.len() >= MAX_SHIPPING_METHODS)
        {
            return Err(OrdersError::TooManyShippingMethods);
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(shipping_methods_id());
                write.original_version(0);
                write
            }
        };
        let method_id = Ulid::generate().to_string();
        write.routing_key_opt(routing_key).event(&MethodAdded {
            method_id: method_id.clone(),
            name: method.name,
            price_cents: method.price_cents,
            currency: method.currency,
            countries: method.countries,
            free_above_cents: method.free_above_cents,
            added_by: actor.to_owned(),
        });
        write.commit(&self.0).await?;
        Ok(method_id)
    }

    /// The merchant describes a method anew. Unchanged is a no-op.
    pub async fn change_shipping_method(
        &self,
        actor: &str,
        method_id: &str,
        method: NewShippingMethod,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        let method = normalize_shipping_method(method)?;
        let state = self
            .load_shipping_methods()
            .await?
            .ok_or(OrdersError::ShippingMethodNotFound)?;
        let current = state
            .method(method_id)
            .ok_or(OrdersError::ShippingMethodNotFound)?;
        if current.name == method.name
            && current.price_cents == method.price_cents
            && current.currency == method.currency
            && current.countries == method.countries
            && current.free_above_cents == method.free_above_cents
        {
            return Ok(());
        }
        let mut write = state.write()?;
        write.routing_key_opt(routing_key).event(&MethodChanged {
            method_id: method_id.to_owned(),
            name: method.name,
            price_cents: method.price_cents,
            currency: method.currency,
            countries: method.countries,
            free_above_cents: method.free_above_cents,
            changed_by: actor.to_owned(),
        });
        write.commit(&self.0).await?;
        Ok(())
    }

    /// The merchant stops offering a method. A shopper who picked it is
    /// asked to pick again when they pay.
    pub async fn remove_shipping_method(
        &self,
        actor: &str,
        method_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        let state = self
            .load_shipping_methods()
            .await?
            .ok_or(OrdersError::ShippingMethodNotFound)?;
        if state.method(method_id).is_none() {
            return Err(OrdersError::ShippingMethodNotFound);
        }
        let mut write = state.write()?;
        write.routing_key_opt(routing_key).event(&MethodRemoved {
            method_id: method_id.to_owned(),
            removed_by: actor.to_owned(),
        });
        write.commit(&self.0).await?;
        Ok(())
    }
}
