//! From `category_path` labels to managed categories: products created
//! before categories existed only carry the breadcrumb they were typed with.
//! [`adopt_category_paths`] opens the categories those breadcrumbs name and
//! files each product under the last one.

use evento::Executor;
use sqlx::SqlitePool;
use timada_core::slug::slugify;

use crate::{
    command::{Command, CreateCategory, category_id},
    error::CatalogError,
    query::load_product_page,
};

/// The category named `name` under `parent`, opened if need be. Its slug is
/// the name's — prefixed with the parent's when another branch already took
/// it ("accessoires" under two parents).
async fn adopt_category<E: Executor>(
    cmd: &Command<'_, E>,
    name: &str,
    parent: Option<&(String, String)>,
) -> Result<Option<(String, String)>, CatalogError> {
    let plain = slugify(name);
    if plain.is_empty() {
        return Ok(None);
    }
    let parent_id = parent.map(|(id, _)| id.clone());
    let candidates = [
        Some(plain.clone()),
        parent.map(|(_, parent_slug)| format!("{parent_slug}-{plain}")),
    ];
    for slug in candidates.into_iter().flatten() {
        let id = category_id(&slug);
        match cmd.load_category(&id).await? {
            Some(existing) if existing.parent_id == parent_id => return Ok(Some((id, slug))),
            Some(_) => continue,
            None => {}
        }
        let created = cmd
            .create_category(CreateCategory {
                name: name.trim().to_owned(),
                slug: Some(slug.clone()),
                parent_id: parent_id.clone(),
            })
            .await;
        match created {
            // Someone opened it in between: look again on the next run.
            Ok(_) | Err(CatalogError::SlugAlreadyExists(_)) => return Ok(Some((id, slug))),
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

/// Files every product that has no category yet under the one its
/// `category_path` names, opening the categories on the way. Reads the
/// product list (`catalog_product`), so it runs once that list is up to date;
/// safe to run again — it only looks at products still unfiled — and returns
/// how many products it filed. A product whose path cannot be adopted (empty,
/// too deep, under an archived category) is left for an operator.
pub async fn adopt_category_paths<E: Executor>(
    executor: &E,
    db: &SqlitePool,
) -> Result<u32, CatalogError> {
    let cmd = Command(executor);
    let unfiled: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM catalog_product WHERE category_id IS NULL AND archived = 0 ORDER BY id",
    )
    .fetch_all(db)
    .await
    .map_err(anyhow::Error::from)?;

    let mut filed = 0;
    for product_id in unfiled {
        let Some(product) = load_product_page(executor, &product_id).await? else {
            continue;
        };
        if product.category_id.is_some() {
            // Filed already; the list has not caught up.
            continue;
        }
        let mut leaf: Option<(String, String)> = None;
        let mut adopted = true;
        for name in &product.category_path {
            match adopt_category(&cmd, name, leaf.as_ref()).await {
                Ok(Some(category)) => leaf = Some(category),
                Ok(None)
                | Err(CatalogError::CategoryArchived | CatalogError::CategoryTooDeep(_)) => {
                    adopted = false;
                    break;
                }
                Err(err) => return Err(err),
            }
        }
        let Some((category_id, _)) = leaf.filter(|_| adopted) else {
            tracing::warn!(%product_id, path = ?product.category_path, "category path not adopted");
            continue;
        };
        match cmd.categorise_product(&product_id, category_id).await {
            Ok(_) => filed += 1,
            Err(CatalogError::ProductArchived | CatalogError::CategoryArchived) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filed)
}
