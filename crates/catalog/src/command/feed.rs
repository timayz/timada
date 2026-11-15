//! What a shopping feed and search engines ask of a product beyond what a
//! shopper reads: who makes it and under which numbers, where it is filed,
//! what its options are, the labels ad campaigns group it by, and whether a
//! machine wrote its text.

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{
    AiWritingDeclared, Classified, FeedLabelsSet, FeedListingSet, Identified, OptionKindSet,
    VariantIdentified,
};
use crate::error::CatalogError;
use crate::value_object::{
    AGE_GROUPS, FEED_LABELS, GENDERS, OPTION_KINDS, known_key, normalize_brand,
    normalize_feed_label, normalize_feed_labels, normalize_google_category, normalize_gtin,
    normalize_mpn,
};

/// What tells a product apart from every other one sold anywhere. The GTIN
/// and the part number are those of a product sold as is: one with variants
/// gives each its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identify {
    pub brand: Option<String>,
    pub gtin: Option<String>,
    pub mpn: Option<String>,
    /// It truly has no GTIN and no part number.
    pub no_identifier: bool,
}

/// Where a shopping feed files a product and whom it is made for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Classify {
    /// A category of Google's product taxonomy: its number or its path.
    pub google_category: Option<String>,
    pub age_group: Option<String>,
    pub gender: Option<String>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Says who makes the product and under which numbers it is known; an
    /// empty field clears it. A product said to have no identifier is
    /// refused a GTIN or a part number, of its own or on a variant. The same
    /// is a no-op.
    pub async fn identify_product(
        &self,
        actor: &str,
        product_id: &str,
        cmd: Identify,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let brand = normalize_brand(cmd.brand.as_deref())?;
        let gtin = normalize_gtin(cmd.gtin.as_deref())?;
        let mpn = normalize_mpn(cmd.mpn.as_deref())?;
        let product = self.require_editable(product_id).await?;
        let numbered = gtin.is_some()
            || mpn.is_some()
            || product
                .variants
                .iter()
                .any(|variant| variant.gtin.is_some() || variant.mpn.is_some());
        if cmd.no_identifier && numbered {
            return Err(CatalogError::IdentifierConflict);
        }
        if product.brand == brand
            && product.gtin == gtin
            && product.mpn == mpn
            && product.no_identifier == cmd.no_identifier
        {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Identified {
                brand,
                gtin,
                mpn,
                no_identifier: cmd.no_identifier,
                identified_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets the GTIN and the part number of one combination; an empty field
    /// clears it. Refused on a product said to have no identifier. The same
    /// is a no-op.
    pub async fn identify_variant(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: &str,
        gtin: Option<&str>,
        mpn: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let gtin = normalize_gtin(gtin)?;
        let mpn = normalize_mpn(mpn)?;
        let product = self.require_editable(product_id).await?;
        let variant = product
            .variant(variant_id)
            .ok_or(CatalogError::VariantNotFound)?;
        if product.no_identifier && (gtin.is_some() || mpn.is_some()) {
            return Err(CatalogError::IdentifierConflict);
        }
        if variant.gtin == gtin && variant.mpn == mpn {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&VariantIdentified {
                variant_id: variant_id.to_owned(),
                gtin,
                mpn,
                identified_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Files the product for a shopping feed: its category in Google's
    /// taxonomy, the ages and the people it is made for. An empty field
    /// clears it; the same is a no-op.
    pub async fn classify_product(
        &self,
        actor: &str,
        product_id: &str,
        cmd: Classify,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let google_category = normalize_google_category(cmd.google_category.as_deref())?;
        let age_group =
            known_key(AGE_GROUPS, cmd.age_group.as_deref()).ok_or(CatalogError::InvalidAudience)?;
        let gender =
            known_key(GENDERS, cmd.gender.as_deref()).ok_or(CatalogError::InvalidAudience)?;
        let product = self.require_editable(product_id).await?;
        if product.google_category == google_category
            && product.age_group == age_group
            && product.gender == gender
        {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Classified {
                google_category,
                age_group,
                gender,
                classified_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Says what an option is to a feed and to search engines: a colour, a
    /// size, a material or a pattern, or none of them with `None`. Refused
    /// when another option of the product is of that kind already. The same
    /// is a no-op.
    pub async fn set_option_kind(
        &self,
        actor: &str,
        product_id: &str,
        option_id: &str,
        kind: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let kind = known_key(OPTION_KINDS, kind).ok_or(CatalogError::InvalidOptionKind)?;
        let product = self.require_editable(product_id).await?;
        let option = product
            .option(option_id)
            .ok_or(CatalogError::OptionNotFound)?;
        if option.kind == kind {
            return Ok(());
        }
        if kind.is_some() && product.options.iter().any(|other| other.kind == kind) {
            return Err(CatalogError::OptionKindTaken);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&OptionKindSet {
                option_id: option_id.to_owned(),
                kind,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets the labels ad campaigns group the product by, the first of
    /// `labels` being label 0; those not given, and the empty ones, are
    /// cleared. The same is a no-op.
    pub async fn set_feed_labels(
        &self,
        actor: &str,
        product_id: &str,
        labels: &[String],
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let labels = normalize_feed_labels(labels)?;
        let product = self.require_editable(product_id).await?;
        if product.feed_labels == labels {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&FeedLabelsSet {
                labels: labels.to_vec(),
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets one label of the product, `index` from 0, leaving the four
    /// others as they are: what labelling many products at once does to
    /// each. An empty label clears it; the same is a no-op.
    pub async fn set_feed_label(
        &self,
        actor: &str,
        product_id: &str,
        index: usize,
        label: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if index >= FEED_LABELS {
            return Err(CatalogError::InvalidFeedLabel);
        }
        let label = normalize_feed_label(label)?;
        let product = self.require_editable(product_id).await?;
        if product.feed_labels[index] == label {
            return Ok(());
        }
        let mut labels = product.feed_labels.to_vec();
        labels[index] = label;
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&FeedLabelsSet {
                labels,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Puts the product in the shop's shopping feed or takes it out; it
    /// stays in the shop either way. The same is a no-op.
    pub async fn list_in_feed(
        &self,
        actor: &str,
        product_id: &str,
        listed: bool,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if product.off_feed != listed {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&FeedListingSet {
                listed,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Says whether a machine wrote the product's name and its description.
    /// The same is a no-op.
    pub async fn declare_ai_writing(
        &self,
        actor: &str,
        product_id: &str,
        title: bool,
        description: bool,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if product.ai_title == title && product.ai_description == description {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&AiWritingDeclared {
                title,
                description,
                declared_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
