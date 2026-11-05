//! The ways a product varies. They are settled before any variant is
//! defined: changing them afterwards would leave a variant naming an option
//! that is gone, or missing one that appeared.

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{
    OptionDefined, OptionHidden, OptionLabelled, OptionRemoved, OptionRenamed, OptionShown,
    OptionValueLabelled, OptionValuesChanged,
};
use crate::error::CatalogError;
use crate::value_object::{
    MAX_OPTIONS, new_part_id, normalize_option_label, normalize_option_name,
    normalize_option_values,
};

/// What shoppers read for an option and its values: the option's label and
/// one per value named, `None` going back to the name or value itself. A
/// value left out keeps the label it has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionLabels {
    pub label: Option<String>,
    pub values: Vec<(String, Option<String>)>,
}

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
            .tenant(&self.tenant)
            .commit(&self.executor)
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
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Replaces the values an option takes. A value a variant still uses, or
    /// the one the option is hidden at, cannot be dropped; the same list is a
    /// no-op.
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
        if option
            .fixed_value
            .as_ref()
            .is_some_and(|fixed| !values.contains(fixed))
        {
            return Err(CatalogError::OptionHiddenAtValue);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&OptionValuesChanged {
                option_id: option_id.to_owned(),
                values,
                changed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets what shoppers read for an option and its values, in one write.
    /// The name and the values themselves do not move, so an import that
    /// named them still finds the option and its combinations: labels are
    /// the one thing about an option that changes while variants are sold.
    /// Labels already as asked write nothing.
    pub async fn label_option(
        &self,
        actor: &str,
        product_id: &str,
        option_id: &str,
        labels: OptionLabels,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let label = normalize_option_label(labels.label.as_deref())?;
        let mut values = Vec::with_capacity(labels.values.len());
        for (value, value_label) in &labels.values {
            values.push((
                value.trim(),
                normalize_option_label(value_label.as_deref())?,
            ));
        }
        let product = self.require_editable(product_id).await?;
        let option = product
            .option(option_id)
            .ok_or(CatalogError::OptionNotFound)?;
        let mut write = product.write()?;
        write.routing_key_opt(routing_key);
        let mut changed = false;
        if option.label != label {
            write.event(&OptionLabelled {
                option_id: option_id.to_owned(),
                label,
                labelled_by: actor.to_owned(),
            });
            changed = true;
        }
        for (value, value_label) in values {
            let value = option
                .values
                .iter()
                .find(|known| known.as_str() == value)
                .ok_or(CatalogError::OptionValueNotFound)?;
            if option.value_labels.get(value) == value_label.as_ref() {
                continue;
            }
            write.event(&OptionValueLabelled {
                option_id: option_id.to_owned(),
                value: value.clone(),
                label: value_label,
                labelled_by: actor.to_owned(),
            });
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }

    /// Hides an option from shoppers at one of its values: the shop sells
    /// only the combinations taking it, without asking, and leaves the option
    /// out of what shoppers read. The variants do not move, so an import that
    /// named them still finds them, and this changes while they are sold, like
    /// labels. Refused at a value that would leave nothing on sale; the same
    /// value is a no-op.
    pub async fn hide_option(
        &self,
        actor: &str,
        product_id: &str,
        option_id: &str,
        value: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let value = value.trim();
        let product = self.require_editable(product_id).await?;
        let option = product
            .option(option_id)
            .ok_or(CatalogError::OptionNotFound)?;
        if !option.values.iter().any(|known| known == value) {
            return Err(CatalogError::OptionValueNotFound);
        }
        if option.fixed_value.as_deref() == Some(value) {
            return Ok(());
        }
        if !product.can_hide_at(option_id, value) {
            return Err(CatalogError::OptionValueNotSold);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&OptionHidden {
                option_id: option_id.to_owned(),
                value: value.to_owned(),
                hidden_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Lets shoppers choose an option again, every combination back on sale.
    /// One they choose already is a no-op.
    pub async fn show_option(
        &self,
        actor: &str,
        product_id: &str,
        option_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        let option = product
            .option(option_id)
            .ok_or(CatalogError::OptionNotFound)?;
        if !option.is_hidden() {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&OptionShown {
                option_id: option_id.to_owned(),
                shown_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
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
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
