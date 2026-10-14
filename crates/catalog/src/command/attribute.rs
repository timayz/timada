//! The facts in a product's table: what it is made of, how big it is.

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{AttributeRemoved, AttributeSet};
use crate::error::CatalogError;
use crate::value_object::{MAX_ATTRIBUTES, normalize_attribute};

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets a fact, replacing the value of one already there under that
    /// name. The same value is a no-op.
    pub async fn set_attribute(
        &self,
        actor: &str,
        product_id: &str,
        name: &str,
        value: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let (name, value) = normalize_attribute(name, value)?;
        let product = self.require_editable(product_id).await?;
        let existing = product
            .attributes
            .iter()
            .find(|attribute| attribute.name == name);
        match existing {
            Some(attribute) if attribute.value == value => return Ok(()),
            None if product.attributes.len() >= MAX_ATTRIBUTES => {
                return Err(CatalogError::TooManyAttributes);
            }
            _ => {}
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&AttributeSet {
                name,
                value,
                set_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Drops a fact; one that is not there is a no-op.
    pub async fn remove_attribute(
        &self,
        actor: &str,
        product_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let name = name.trim();
        let product = self.require_editable(product_id).await?;
        if !product
            .attributes
            .iter()
            .any(|attribute| attribute.name == name)
        {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&AttributeRemoved {
                name: name.to_owned(),
                removed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
