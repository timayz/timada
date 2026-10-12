use evento::Executor;
use ulid::Ulid;

use crate::aggregator::Created;
use crate::error::CatalogError;
use crate::value_object::{normalize_name, normalize_slug, slugify, suffixed_slug};

use super::ProductState;

/// Tries at the derived slug, then with fresh random suffixes.
const SLUG_ATTEMPTS: usize = 4;

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
    /// product stream that cannot be created after the claim gives the slug
    /// back (best effort: a failure there only leaves the slug reserved).
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
                self.claim_slug(&slug, &product_id, routing_key.clone())
                    .await?;
                slug
            }
            None => {
                let base = slugify(&name);
                let mut slug = base.clone();
                for attempt in 0..SLUG_ATTEMPTS {
                    match self
                        .claim_slug(&slug, &product_id, routing_key.clone())
                        .await
                    {
                        Ok(()) => break,
                        Err(CatalogError::SlugTaken) if attempt + 1 < SLUG_ATTEMPTS => {
                            slug = suffixed_slug(&base);
                        }
                        Err(err) => return Err(err),
                    }
                }
                slug
            }
        };

        let created = evento::append(&product_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Created {
                slug: slug.clone(),
                name,
                created_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await;
        if let Err(err) = created {
            if let Err(release) = self.release_slug(&slug, &product_id, routing_key).await {
                tracing::warn!(%slug, %release, "cannot give the slug back after a failed creation");
            }
            return Err(err.into());
        }

        self.load_product(&product_id)
            .await?
            .ok_or(CatalogError::NotFound)
    }
}
