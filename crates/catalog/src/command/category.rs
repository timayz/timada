//! The category tree: create, edit, move, order and archive a node.

use evento::{Executor, ProjectionAggregate};
use ulid::Ulid;

use crate::aggregator::category::{
    Archived, BodyWritten, Created, Described, HiddenFromMenu, Moved, Positioned, Renamed,
    ShownInMenu,
};
use crate::error::CatalogError;
use crate::value_object::{
    FALLBACK_CATEGORY_SLUG, MAX_CATEGORY_DEPTH, normalize_category_body,
    normalize_category_description, normalize_category_name, normalize_slug, slugify_or,
    suffixed_slug,
};

use super::CategoryState;

/// Tries at the derived slug, then with fresh random suffixes.
const SLUG_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateCategory {
    /// The display name.
    pub name: String,
    /// The URL handle; derived from the name when `None`. A given slug that
    /// is taken is refused, a derived one gets a suffix.
    pub slug: Option<String>,
    /// The category above it, or `None` for a top-level one.
    pub parent_id: Option<String>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Creates a category: checks the parent leaves room below it, claims the
    /// slug, then appends `Created`. A stream that cannot be created after
    /// the claim gives the slug back (best effort).
    pub async fn create_category(
        &self,
        actor: &str,
        cmd: CreateCategory,
        routing_key: Option<String>,
    ) -> Result<CategoryState, CatalogError> {
        let name = normalize_category_name(&cmd.name)?;
        let parent_id = cmd
            .parent_id
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned);
        if let Some(parent_id) = &parent_id {
            let chain = self.parent_chain(parent_id).await?;
            if chain.len() + 1 > MAX_CATEGORY_DEPTH {
                return Err(CatalogError::CategoryTooDeep);
            }
        }
        let category_id = Ulid::generate().to_string();

        let slug = match cmd.slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(given) => {
                let slug = normalize_slug(given)?;
                self.claim_category_slug(&slug, &category_id, routing_key.clone())
                    .await?;
                slug
            }
            None => {
                let base = slugify_or(&name, FALLBACK_CATEGORY_SLUG);
                let mut slug = base.clone();
                for attempt in 0..SLUG_ATTEMPTS {
                    match self
                        .claim_category_slug(&slug, &category_id, routing_key.clone())
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

        let created = evento::append(&category_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Created {
                slug: slug.clone(),
                name,
                parent_id,
                created_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        if let Err(err) = created {
            if let Err(release) = self
                .release_category_slug(&slug, &category_id, routing_key)
                .await
            {
                tracing::warn!(%slug, %release, "cannot give the category slug back after a failed creation");
            }
            return Err(err.into());
        }

        self.load_category(&category_id)
            .await?
            .ok_or(CatalogError::CategoryNotFound)
    }

    /// Changes the display name; the same name is a no-op.
    pub async fn rename_category(
        &self,
        actor: &str,
        category_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let name = normalize_category_name(name)?;
        let category = self.require_live_category(category_id).await?;
        if category.name == name {
            return Ok(());
        }
        category
            .write()?
            .routing_key_opt(routing_key)
            .event(&Renamed {
                name,
                renamed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Replaces the description; the same text is a no-op.
    pub async fn describe_category(
        &self,
        actor: &str,
        category_id: &str,
        description: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let description = normalize_category_description(description)?;
        let category = self.require_live_category(category_id).await?;
        if category.description == description {
            return Ok(());
        }
        category
            .write()?
            .routing_key_opt(routing_key)
            .event(&Described {
                description,
                described_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Replaces the text below the category's products; the same text is a
    /// no-op.
    pub async fn write_category_body(
        &self,
        actor: &str,
        category_id: &str,
        body: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let body = normalize_category_body(body)?;
        let category = self.require_live_category(category_id).await?;
        if category.body == body {
            return Ok(());
        }
        category
            .write()?
            .routing_key_opt(routing_key)
            .event(&BodyWritten {
                body,
                written_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Puts the category under another one, or at the top level with `None`.
    /// Refused under itself or one of its descendants, and where the tree
    /// would go deeper than [`MAX_CATEGORY_DEPTH`]. The same parent is a
    /// no-op.
    pub async fn move_category(
        &self,
        actor: &str,
        category_id: &str,
        parent_id: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let parent_id = parent_id.map(str::trim).filter(|p| !p.is_empty());
        let category = self.require_live_category(category_id).await?;
        if category.parent_id.as_deref() == parent_id {
            return Ok(());
        }
        if let Some(parent_id) = parent_id {
            if parent_id == category_id {
                return Err(CatalogError::CategoryCycle);
            }
            let chain = self.parent_chain(parent_id).await?;
            // A loop first: it is the better answer when the new parent is
            // both below the category and at the bottom of the tree.
            if chain.iter().any(|ancestor| ancestor.id == category_id) {
                return Err(CatalogError::CategoryCycle);
            }
            if chain.len() + 1 > MAX_CATEGORY_DEPTH {
                return Err(CatalogError::CategoryTooDeep);
            }
        }
        category
            .write()?
            .routing_key_opt(routing_key)
            .event(&Moved {
                parent_id: parent_id.map(str::to_owned),
                moved_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets where the category sorts among its siblings; the same position
    /// is a no-op.
    pub async fn position_category(
        &self,
        actor: &str,
        category_id: &str,
        position: u32,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let category = self.require_live_category(category_id).await?;
        if category.position == position {
            return Ok(());
        }
        category
            .write()?
            .routing_key_opt(routing_key)
            .event(&Positioned {
                position,
                positioned_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Puts the category in the shop's menu or leaves it out, with its
    /// subtree; asking for what it already is is a no-op.
    pub async fn set_category_in_menu(
        &self,
        actor: &str,
        category_id: &str,
        in_menu: bool,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let category = self.require_live_category(category_id).await?;
        if category.hidden_from_menu != in_menu {
            return Ok(());
        }
        let mut write = category.write()?;
        let write = write.routing_key_opt(routing_key);
        if in_menu {
            write.event(&ShownInMenu {
                shown_by: actor.to_owned(),
            });
        } else {
            write.event(&HiddenFromMenu {
                hidden_by: actor.to_owned(),
            });
        }
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }

    /// Retires the category for good and frees its slug. Already archived
    /// is a no-op. Its children and products are left as they are.
    pub async fn archive_category(
        &self,
        actor: &str,
        category_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let category = match self.require_live_category(category_id).await {
            Ok(category) => category,
            Err(CatalogError::CategoryArchived) => return Ok(()),
            Err(err) => return Err(err),
        };
        category
            .write()?
            .routing_key_opt(routing_key.clone())
            .event(&Archived {
                archived_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        if let Err(release) = self
            .release_category_slug(&category.slug, category_id, routing_key)
            .await
        {
            tracing::warn!(slug = %category.slug, %release, "cannot give the category slug back after archiving");
        }
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    /// A live parent and its ancestors, top-level first and the parent last,
    /// so its length is the depth a new child would sit at minus one.
    async fn parent_chain(&self, parent_id: &str) -> Result<Vec<CategoryState>, CatalogError> {
        let parent = self.require_live_category(parent_id).await?;
        let mut chain = self.ancestors(parent_id).await?;
        chain.push(parent);
        Ok(chain)
    }
}
