use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{
        CategoryArchived, CategoryCreated, CategoryDescribed, CategoryMoved, CategoryPositioned,
        CategoryRenamed,
    },
    error::CatalogError,
};

use super::category_id;

/// How deep the tree goes, root categories being level 1.
pub const MAX_CATEGORY_DEPTH: usize = 6;

#[derive(Debug, Clone)]
pub struct CreateCategory {
    pub name: String,
    /// The URL word of the category, for ever; derived from the name when
    /// left empty.
    pub slug: Option<String>,
    pub parent_id: Option<String>,
}

impl<E: Executor> super::Command<'_, E> {
    /// The ids from `id` up to its root, `id` first. Stops at
    /// [`MAX_CATEGORY_DEPTH`]: a tree that deep was refused when it was built.
    async fn category_lineage(&self, id: &str) -> Result<Vec<String>, CatalogError> {
        let mut lineage = vec![id.to_owned()];
        let mut current = self
            .load_category(id)
            .await?
            .ok_or(CatalogError::CategoryNotFound)?;
        while let Some(parent_id) = current.parent_id.take() {
            if lineage.len() > MAX_CATEGORY_DEPTH || lineage.contains(&parent_id) {
                break;
            }
            let Some(parent) = self.load_category(&parent_id).await? else {
                break;
            };
            lineage.push(parent_id);
            current = parent;
        }
        Ok(lineage)
    }

    /// Opens a category. Its id derives from the slug, so a second category
    /// with the same slug is refused atomically by the store.
    pub async fn create_category(&self, cmd: CreateCategory) -> Result<String, CatalogError> {
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
        if let Some(parent_id) = &cmd.parent_id {
            self.load_open_category(parent_id).await?;
            if self.category_lineage(parent_id).await?.len() >= MAX_CATEGORY_DEPTH {
                return Err(CatalogError::CategoryTooDeep(MAX_CATEGORY_DEPTH));
            }
        }

        let result = evento::append(category_id(&slug))
            .event(&CategoryCreated {
                slug: slug.clone(),
                name,
                parent_id: cmd.parent_id,
            })
            .commit(self.0)
            .await;
        match result {
            Ok(id) => {
                tracing::info!(category_id = %id, %slug, "category created");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(CatalogError::SlugAlreadyExists(slug))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn rename_category(
        &self,
        id: impl Into<String>,
        name: &str,
    ) -> Result<(), CatalogError> {
        let category = self.load_open_category(id).await?;
        let name = name.trim();
        if name.is_empty() {
            return Err(CatalogError::Required("name"));
        }
        if category.name == name {
            return Ok(());
        }
        category
            .write()?
            .event(&CategoryRenamed {
                name: name.to_owned(),
            })
            .commit(self.0)
            .await?;
        Ok(())
    }

    pub async fn describe_category(
        &self,
        id: impl Into<String>,
        description: &str,
    ) -> Result<(), CatalogError> {
        let category = self.load_open_category(id).await?;
        let description = description.trim();
        if category.description == description {
            return Ok(());
        }
        category
            .write()?
            .event(&CategoryDescribed {
                description: description.to_owned(),
            })
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// Moves a category, with everything under it, under another parent — or
    /// to the root. Never under itself or one of its own subcategories.
    pub async fn move_category(
        &self,
        id: impl Into<String>,
        parent_id: Option<String>,
    ) -> Result<(), CatalogError> {
        let category = self.load_open_category(id).await?;
        if category.parent_id == parent_id {
            return Ok(());
        }
        if let Some(parent_id) = &parent_id {
            self.load_open_category(parent_id).await?;
            let lineage = self.category_lineage(parent_id).await?;
            if lineage.contains(&category.id) {
                return Err(CatalogError::CategoryCycle);
            }
            if lineage.len() >= MAX_CATEGORY_DEPTH {
                return Err(CatalogError::CategoryTooDeep(MAX_CATEGORY_DEPTH));
            }
        }
        category
            .write()?
            .event(&CategoryMoved { parent_id })
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// Ranks a category among its siblings, lowest first; ties go by name.
    pub async fn position_category(
        &self,
        id: impl Into<String>,
        position: u32,
    ) -> Result<(), CatalogError> {
        let category = self.load_open_category(id).await?;
        if category.position == position {
            return Ok(());
        }
        category
            .write()?
            .event(&CategoryPositioned { position })
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// Takes a category off the storefront, with everything under it. Its
    /// products stay what they are — and can be filed elsewhere. Archiving
    /// twice is harmless.
    pub async fn archive_category(&self, id: impl Into<String>) -> Result<(), CatalogError> {
        let Some(category) = self.load_category(id).await? else {
            return Err(CatalogError::CategoryNotFound);
        };
        if category.archived {
            return Ok(());
        }
        category
            .write()?
            .event(&CategoryArchived)
            .commit(self.0)
            .await?;
        Ok(())
    }
}
