use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{
        FamilyCreated, FamilyDissolved, FamilyOptionsDefined, FamilyRenamed, FamilyVariantPlaced,
        FamilyVariantRemoved, ProductJoinedFamily, ProductLeftFamily,
    },
    error::CatalogError,
    value_object::{FamilyOption, OptionValue},
};

use super::{FamilyState, family_id};

/// How many options tell the variants of a family apart.
pub const MAX_FAMILY_OPTIONS: usize = 3;

/// How many values one option takes.
pub const MAX_OPTION_VALUES: usize = 50;

#[derive(Debug, Clone)]
pub struct CreateFamily {
    pub name: String,
    /// The family's permanent word; derived from the name when left empty.
    pub slug: Option<String>,
}

/// Trimmed, blanks and repeats dropped. An option named twice is refused
/// rather than merged: which list of values was meant?
fn tidy_options(options: Vec<FamilyOption>) -> Result<Vec<FamilyOption>, CatalogError> {
    let mut kept: Vec<FamilyOption> = Vec::new();
    for option in options {
        let name = option.name.trim().to_owned();
        if name.is_empty() {
            continue;
        }
        if kept.iter().any(|other| other.name == name) {
            return Err(CatalogError::DuplicateOption(name));
        }
        let mut values: Vec<String> = Vec::new();
        for value in option.values {
            let value = value.trim().to_owned();
            if !value.is_empty() && !values.contains(&value) {
                values.push(value);
            }
        }
        if values.len() > MAX_OPTION_VALUES {
            return Err(CatalogError::TooManyOptionValues(MAX_OPTION_VALUES));
        }
        kept.push(FamilyOption { name, values });
    }
    if kept.len() > MAX_FAMILY_OPTIONS {
        return Err(CatalogError::TooManyOptions(MAX_FAMILY_OPTIONS));
    }
    Ok(kept)
}

/// The place `values` describe in `family`: one offered value per option, in
/// the options' order; anything said about another option is dropped.
fn place_in(
    family: &FamilyState,
    values: &[OptionValue],
) -> Result<Vec<OptionValue>, CatalogError> {
    if family.options.is_empty() {
        return Err(CatalogError::Required("options"));
    }
    family
        .options
        .iter()
        .map(|option| {
            let value = values
                .iter()
                .find(|given| given.option.trim() == option.name)
                .map(|given| given.value.trim())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| CatalogError::MissingOptionValue(option.name.clone()))?;
            if !option.values.iter().any(|offered| offered == value) {
                return Err(CatalogError::UnknownOptionValue {
                    option: option.name.clone(),
                    value: value.to_owned(),
                });
            }
            Ok(OptionValue::new(&option.name, value))
        })
        .collect()
}

impl<E: Executor> super::Command<'_, E> {
    /// Opens a product family. Its id derives from the slug, so a second
    /// family with the same slug is refused atomically by the store.
    pub async fn create_family(&self, cmd: CreateFamily) -> Result<String, CatalogError> {
        let name = cmd.name.trim().to_owned();
        if name.is_empty() {
            return Err(CatalogError::Required("name"));
        }
        let slug = match cmd.slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(slug) if timada_core::slug::is_slug(slug) => slug.to_owned(),
            Some(slug) => return Err(CatalogError::InvalidSlug(slug.to_owned())),
            None => timada_core::slug::slugify(&name),
        };
        if slug.is_empty() {
            return Err(CatalogError::Required("slug"));
        }

        let result = evento::append(family_id(&slug))
            .event(&FamilyCreated {
                slug: slug.clone(),
                name,
            })
            .commit(self.0)
            .await;
        match result {
            Ok(id) => {
                tracing::info!(family_id = %id, %slug, "product family created");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(CatalogError::FamilySlugAlreadyExists(slug))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn rename_family(
        &self,
        id: impl Into<String>,
        name: &str,
    ) -> Result<(), CatalogError> {
        let family = self.load_open_family(id).await?;
        let name = name.trim();
        if name.is_empty() {
            return Err(CatalogError::Required("name"));
        }
        if family.name == name {
            return Ok(());
        }
        family
            .write()?
            .event(&FamilyRenamed {
                name: name.to_owned(),
            })
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// Says what tells the variants apart and which values each option takes,
    /// in the order shown — the whole list. Options and values can be added,
    /// reordered and, while no variant stands on them, taken away; the same
    /// list records nothing.
    pub async fn define_family_options(
        &self,
        id: impl Into<String>,
        options: Vec<FamilyOption>,
    ) -> Result<(), CatalogError> {
        let family = self.load_open_family(id).await?;
        let options = tidy_options(options)?;
        if family.options == options {
            return Ok(());
        }
        for placed in family.variants.iter().flat_map(|variant| &variant.values) {
            let still_offered = options
                .iter()
                .find(|option| option.name == placed.option)
                .is_some_and(|option| option.values.contains(&placed.value));
            if !still_offered {
                return Err(CatalogError::OptionInUse {
                    option: placed.option.clone(),
                    value: placed.value.clone(),
                });
            }
        }
        family
            .write()?
            .event(&FamilyOptionsDefined { options })
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// Makes a product a variant of the family, standing at `values` — or
    /// moves a variant there. Returns whether anything changed.
    ///
    /// Two aggregates, two writes. The product says first which family it is
    /// in — what keeps it out of any other — then the family records the
    /// place, which keeps two variants from standing on the same one. Should
    /// the second write not happen, asking again finishes the job.
    pub async fn place_variant(
        &self,
        id: impl Into<String>,
        product_id: impl Into<String>,
        values: Vec<OptionValue>,
    ) -> Result<bool, CatalogError> {
        let family = self.load_open_family(id).await?;
        let product = self.load_active(product_id).await?;
        let values = place_in(&family, &values)?;
        if product
            .family_id
            .as_ref()
            .is_some_and(|other| *other != family.id)
        {
            return Err(CatalogError::ProductInAnotherFamily);
        }
        let taken = family
            .variants
            .iter()
            .any(|variant| variant.product_id != product.id && variant.values == values);
        if taken {
            return Err(CatalogError::VariantPlaceTaken);
        }
        let already_there = family
            .variant(&product.id)
            .is_some_and(|variant| variant.values == values);
        if already_there && product.family_id.is_some() {
            return Ok(false);
        }

        let product_id = product.id.clone();
        if product.family_id.is_none() {
            product
                .write()?
                .event(&ProductJoinedFamily {
                    family_id: family.id.clone(),
                })
                .commit(self.0)
                .await?;
        }
        if !already_there {
            family
                .write()?
                .event(&FamilyVariantPlaced { product_id, values })
                .commit(self.0)
                .await?;
        }
        Ok(true)
    }

    /// Takes a product out of the family: it is on its own again, and may
    /// join another. The family forgets it first, then the product is freed;
    /// asking again finishes what a failure left half done. Returns whether
    /// anything changed.
    pub async fn remove_variant(
        &self,
        id: impl Into<String>,
        product_id: impl Into<String>,
    ) -> Result<bool, CatalogError> {
        let product_id = product_id.into();
        let Some(family) = self.load_family(id).await? else {
            return Err(CatalogError::FamilyNotFound);
        };
        let Some(product) = self.load(&product_id).await? else {
            return Err(CatalogError::ProductNotFound);
        };
        let placed = family.variant(&product_id).is_some();
        let claimed = product.family_id.as_deref() == Some(family.id.as_str());
        if placed {
            family
                .write()?
                .event(&FamilyVariantRemoved { product_id })
                .commit(self.0)
                .await?;
        }
        if claimed {
            product
                .write()?
                .event(&ProductLeftFamily)
                .commit(self.0)
                .await?;
        }
        Ok(placed || claimed)
    }

    /// Ends a family that holds no variant any more. Dissolving twice is
    /// harmless.
    pub async fn dissolve_family(&self, id: impl Into<String>) -> Result<(), CatalogError> {
        let Some(family) = self.load_family(id).await? else {
            return Err(CatalogError::FamilyNotFound);
        };
        if family.dissolved {
            return Ok(());
        }
        if !family.variants.is_empty() {
            return Err(CatalogError::FamilyNotEmpty);
        }
        family
            .write()?
            .event(&FamilyDissolved)
            .commit(self.0)
            .await?;
        Ok(())
    }
}
