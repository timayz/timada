use evento::Executor;
use ulid::Ulid;

use crate::aggregator::Created;
use crate::error::CatalogError;
use crate::value_object::{normalize_name, normalize_slug, slugify};

use super::ProductState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProduct {
    /// The display name.
    pub name: String,
    /// The URL handle; derived from the name when `None`. A given slug that
    /// is taken is refused, a derived one gets a suffix.
    pub slug: Option<String>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Creates a draft product: claims its slug, then appends `Created`. A
    /// creation that fails keeps the claim, as its write may land yet (the
    /// store could not tell): a claim whose product never got written is
    /// taken over once it is no longer fresh.
    pub async fn create_product(
        &self,
        actor: &str,
        cmd: CreateProduct,
        routing_key: Option<String>,
    ) -> Result<ProductState, CatalogError> {
        let name = normalize_name(&cmd.name)?;
        let product_id = Ulid::generate().to_string();

        let slug = match cmd.slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(given) => {
                let slug = normalize_slug(given)?;
                self.claim_slug(&slug, &product_id, 0, routing_key.clone())
                    .await?;
                slug
            }
            None => {
                self.claim_derived_slug(slugify(&name), &product_id, 0, routing_key.clone())
                    .await?
            }
        };

        evento::append(&product_id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                slug,
                name,
                created_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;

        self.load_product(&product_id)
            .await?
            .ok_or(CatalogError::NotFound)
    }
}
