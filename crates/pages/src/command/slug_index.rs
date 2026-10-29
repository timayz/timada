use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Claimed, Released};
use crate::error::PagesError;
use crate::value_object::page_slug_id;

use super::Command;

impl<E: Executor> Command<E> {
    /// Points a normalized slug at `page_id`, creating or re-claiming the
    /// index entry. Fails when it already points at another page.
    pub(crate) async fn claim_page_slug(
        &self,
        slug: &str,
        page_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PagesError> {
        let claimed = Claimed {
            page_id: page_id.to_owned(),
        };
        let created = evento::append(page_slug_id(slug))
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&claimed)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_page_slug_index_any(slug).await? else {
                    return Err(PagesError::SlugTaken);
                };
                if existing.released {
                    existing
                        .write()?
                        .routing_key_opt(routing_key)
                        .event(&claimed)
                        .tenant(&self.tenant)
                        .commit(&self.executor)
                        .await?;
                    Ok(())
                } else if existing.page_id == page_id {
                    Ok(())
                } else {
                    Err(PagesError::SlugTaken)
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Frees the index entry of a slug owned by `page_id`. A missing or
    /// already freed entry is fine; one owned by another page is left alone.
    pub(crate) async fn release_page_slug(
        &self,
        slug: &str,
        page_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PagesError> {
        if let Some(index) = self.load_page_slug_index(slug).await?
            && index.page_id == page_id
        {
            index
                .write()?
                .routing_key_opt(routing_key)
                .event(&Released)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await?;
        }
        Ok(())
    }
}
