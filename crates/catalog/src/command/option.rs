//! The ways a product varies. They are settled before any variant is
//! defined: changing them afterwards would leave a variant naming an option
//! that is gone, or missing one that appeared.

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{OptionDefined, OptionRemoved, OptionRenamed, OptionValuesChanged};
use crate::error::CatalogError;
use crate::value_object::{
    MAX_OPTIONS, new_part_id, normalize_option_name, normalize_option_values,
};

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Adds a way the product varies, with the values it takes, and returns
    /// its id.
    pub async fn define_option(
        &self,
        actor: &str,
        product_id: &str,
        name: &str,
        values: Vec<String>,
        routing_key: Option<String>,
    ) -> Result<String, CatalogError> {
        let name = normalize_option_name(name)?;
        let values = normalize_option_values(&values)?;
        let product = self.require_editable(product_id).await?;
        if !product.variants.is_empty() {
            return Err(CatalogError::OptionInUse);
        }
        if product.options.len() >= MAX_OPTIONS {
            return Err(CatalogError::TooManyOptions);
        }
        if product
            .options
            .iter()
            .any(|option| option.name.eq_ignore_ascii_case(&name))
        {
            return Err(CatalogError::OptionExists);
        }
        let option_id = new_part_id();
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&OptionDefined {
                option_id: option_id.clone(),
                name,
                values,
                defined_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(option_id)
    }

    /// Renames an option; the same name is a no-op.
    pub async fn rename_option(
        &self,
        actor: &str,
        product_id: &str,
        option_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let name = normalize_option_name(name)?;
        let product = self.require_editable(product_id).await?;
        let option = product
            .option(option_id)
            .ok_or(CatalogError::OptionNotFound)?;
        if option.name == name {
            return Ok(());
        }
        if product
            .options
            .iter()
            .any(|other| other.id != option_id && other.name.eq_ignore_ascii_case(&name))
        {
            return Err(CatalogError::OptionExists);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&OptionRenamed {
                option_id: option_id.to_owned(),
                name,
                renamed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Replaces the values an option takes. A value a variant still uses
    /// cannot be dropped; the same list is a no-op.
    pub async fn change_option_values(
        &self,
        actor: &str,
        product_id: &str,
        option_id: &str,
        values: Vec<String>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let values = normalize_option_values(&values)?;
        let product = self.require_editable(product_id).await?;
        let option = product
            .option(option_id)
            .ok_or(CatalogError::OptionNotFound)?;
        if option.values == values {
            return Ok(());
        }
        let in_use = product.variants.iter().any(|variant| {
            variant
                .value_of(option_id)
                .is_some_and(|used| !values.iter().any(|value| value == used))
        });
        if in_use {
            return Err(CatalogError::OptionValueInUse);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&OptionValuesChanged {
                option_id: option_id.to_owned(),
                values,
                changed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Drops a way the product varies. Refused while it sells variants, and
    /// an option that is not there is a no-op.
    pub async fn remove_option(
        &self,
        actor: &str,
        product_id: &str,
        option_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if product.option(option_id).is_none() {
            return Ok(());
        }
        if !product.variants.is_empty() {
            return Err(CatalogError::OptionInUse);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&OptionRemoved {
                option_id: option_id.to_owned(),
                removed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
