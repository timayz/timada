//! Where an old address went. A product or a page renamed in the back
//! office leaves its slug; the copy remembers the slugs each one left
//! (`storefront_former_slugs`), and an archived product's last one, so the
//! old address can answer with a permanent redirect instead of a 404:
//! [`moved_product`] and [`moved_page`]. A live slug always wins: the host
//! asks only once its own page answered 404, so a slug something else
//! claimed since shows that thing.
//!
//! The merchant's own list ([`redirect_for`]) covers the addresses a rename
//! here never knew: those of a shop moved from another platform.

use evento::{Executor, metadata::RawEvent, subscription::Context};
use timada_core::FormerSlug;
use timada_core::db::{Connection, Db, execute, fetch_all};
use timada_pages::aggregator::Redirects;
use timada_pages::redirects_projection;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::store;
use crate::query::published_product_categories;

/// The `kind` of a product's rows in `storefront_former_slugs`.
pub(crate) const PRODUCT: &str = "product";
/// The `kind` of a page's rows.
pub(crate) const PAGE: &str = "page";

/// Makes the former slugs of one thing in the copy what `wanted` says,
/// writing only when they differ: most events of a product move none.
pub(crate) async fn write_former_slugs(
    tx: &Connection,
    kind: &str,
    id: &str,
    wanted: &[FormerSlug],
) -> anyhow::Result<()> {
    let mut wanted: Vec<(String, i64)> = wanted
        .iter()
        .map(|was| (was.slug.clone(), was.left_at as i64))
        .collect();
    wanted.sort();
    let were: Vec<(String, i64)> = fetch_all(
        tx,
        "SELECT slug, left_at FROM storefront_former_slugs
         WHERE kind = ?1 AND id = ?2 ORDER BY slug, left_at",
        (kind, id),
    )
    .await?;
    if were == wanted {
        return Ok(());
    }
    execute(
        tx,
        "DELETE FROM storefront_former_slugs WHERE kind = ?1 AND id = ?2",
        (kind, id),
    )
    .await?;
    for (slug, left_at) in &wanted {
        execute(
            tx,
            "INSERT INTO storefront_former_slugs (kind, slug, id, left_at)
             VALUES (?1, ?2, ?3, ?4)",
            (kind, slug.as_str(), id, *left_at),
        )
        .await?;
    }
    Ok(())
}

/// Where the product that went by a slug is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MovedProduct {
    /// On sale under this slug.
    Product(String),
    /// Archived: the slug of the first live category it was filed under,
    /// where a shopper finds what replaced it.
    Category(String),
}

/// Where the product that last went by `slug` is: the published one that
/// left it most recently, else the category of the archived one that did.
/// `None` when no product left that slug, or the one that did is a draft,
/// deleted, or archived out of every live category.
pub async fn moved_product_slug(db: &Db, slug: &str) -> anyhow::Result<Option<MovedProduct>> {
    let left: Vec<(String, String, String)> = db
        .fetch_all(
            "SELECT p.id, p.slug, p.status
             FROM storefront_former_slugs f
             JOIN storefront_products p ON p.id = f.id
             WHERE f.kind = ?1 AND f.slug = ?2
             ORDER BY f.left_at DESC, f.id",
            (PRODUCT, slug),
        )
        .await?;
    if let Some((_, now, _)) = left.iter().find(|(_, _, status)| status == "published") {
        return Ok(Some(MovedProduct::Product(now.clone())));
    }
    for (id, _, status) in &left {
        if status != "archived" {
            continue;
        }
        if let Some(category) = published_product_categories(db, id)
            .await?
            .into_iter()
            .next()
        {
            return Ok(Some(MovedProduct::Category(category.slug)));
        }
    }
    Ok(None)
}

/// The slug of the published page that last went by `slug`, if any.
pub async fn moved_page_slug(db: &Db, slug: &str) -> anyhow::Result<Option<String>> {
    let now: Option<(String,)> = db
        .fetch_optional(
            "SELECT p.slug
             FROM storefront_former_slugs f
             JOIN storefront_pages p ON p.id = f.id
             WHERE f.kind = ?1 AND f.slug = ?2
             ORDER BY f.left_at DESC, f.id LIMIT 1",
            (PAGE, slug),
        )
        .await?;
    Ok(now.map(|(slug,)| slug))
}

/// Where the merchant's list sends the old path `path` (no query), if it
/// names it. A trailing slash is the same path.
pub async fn redirect_target(db: &Db, path: &str) -> anyhow::Result<Option<String>> {
    let path = match path.trim_end_matches('/') {
        "" => return Ok(None),
        path => path,
    };
    let to: Option<(String,)> = db
        .fetch_optional(
            "SELECT to_path FROM storefront_redirects WHERE from_path = ?1",
            (path,),
        )
        .await?;
    Ok(to.map(|(to,)| to))
}

/// Where the product that went by `slug` is now; see
/// [`moved_product_slug`]. For a product page that found nothing.
pub async fn moved_product(cx: &Cx, slug: &str) -> Result<Option<MovedProduct>> {
    moved_product_slug(store(cx).db(), slug)
        .await
        .map_err(Error::from_anyhow)
}

/// The slug of the published page that went by `slug`; see
/// [`moved_page_slug`].
pub async fn moved_page(cx: &Cx, slug: &str) -> Result<Option<String>> {
    moved_page_slug(store(cx).db(), slug)
        .await
        .map_err(Error::from_anyhow)
}

/// Where the merchant's list sends `path`; see [`redirect_target`].
pub async fn redirect_for(cx: &Cx, path: &str) -> Result<Option<String>> {
    redirect_target(store(cx).db(), path)
        .await
        .map_err(Error::from_anyhow)
}

/// Rewrites the copy of the merchant's list whole: a short one.
#[evento::subscription_all]
pub(crate) async fn on_redirects_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Redirects>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let redirects = redirects_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .map(|state| state.redirects)
        .unwrap_or_default();
    db.write(&redirects, |tx, redirects| {
        Box::pin(async move {
            execute(tx, "DELETE FROM storefront_redirects", ()).await?;
            for redirect in redirects {
                execute(
                    tx,
                    "INSERT INTO storefront_redirects (from_path, to_path) VALUES (?1, ?2)",
                    (redirect.from.as_str(), redirect.to.as_str()),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> Db {
        let db = Db::from(
            timada_core::db::Turso::in_memory_concurrent()
                .await
                .unwrap(),
        );
        crate::migrate(&db).await.unwrap();
        db
    }

    async fn product(db: &Db, id: &str, slug: &str, status: &str) {
        db.execute(
            "INSERT INTO storefront_products (id, slug, name, description, status, updated_at,
                images_json, options_json, variants_json, attributes_json, slug_search,
                name_search)
             VALUES (?1, ?2, ?1, '', ?3, 1, '[]', '[]', '[]', '[]', ?2, ?1)",
            (id, slug, status),
        )
        .await
        .unwrap();
    }

    async fn left(db: &Db, kind: &str, id: &str, slugs: &[(&str, u64)]) {
        let former: Vec<FormerSlug> = slugs
            .iter()
            .map(|(slug, left_at)| FormerSlug {
                slug: (*slug).to_owned(),
                left_at: *left_at,
            })
            .collect();
        let id = id.to_owned();
        let kind = kind.to_owned();
        db.write(&(kind, id, former), |tx, (kind, id, former)| {
            Box::pin(async move { write_former_slugs(tx, kind, id, former).await })
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_old_slug_leads_to_the_product_that_left_it_last() {
        let db = db().await;
        assert_eq!(moved_product_slug(&db, "mug").await.unwrap(), None);
        product(&db, "a", "cup", "published").await;
        left(&db, PRODUCT, "a", &[("mug", 10)]).await;
        assert_eq!(
            moved_product_slug(&db, "mug").await.unwrap(),
            Some(MovedProduct::Product("cup".into()))
        );
        // Another product took the slug and left it later: it is the one.
        product(&db, "b", "pot", "published").await;
        left(&db, PRODUCT, "b", &[("mug", 20)]).await;
        assert_eq!(
            moved_product_slug(&db, "mug").await.unwrap(),
            Some(MovedProduct::Product("pot".into()))
        );
        // Unless it is no longer on sale: the older leaver still is.
        db.execute(
            "UPDATE storefront_products SET status = 'draft' WHERE id = 'b'",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            moved_product_slug(&db, "mug").await.unwrap(),
            Some(MovedProduct::Product("cup".into()))
        );
        // The list follows the state: a slug taken back is forgotten.
        left(&db, PRODUCT, "a", &[]).await;
        assert_eq!(moved_product_slug(&db, "mug").await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_archived_product_leads_to_its_live_category() {
        let db = db().await;
        product(&db, "a", "mug", "archived").await;
        left(&db, PRODUCT, "a", &[("mug", 10)]).await;
        assert_eq!(
            moved_product_slug(&db, "mug").await.unwrap(),
            None,
            "filed nowhere, it leads nowhere"
        );
        db.execute(
            "INSERT INTO storefront_categories (id, slug, name, description, body, position,
                archived, in_menu, updated_at)
             VALUES ('c', 'mugs', 'Mugs', '', '', 0, 0, 1, 1)",
            (),
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO storefront_product_categories (product_id, category_id)
             VALUES ('a', 'c')",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            moved_product_slug(&db, "mug").await.unwrap(),
            Some(MovedProduct::Category("mugs".into()))
        );
    }

    #[tokio::test]
    async fn an_old_page_slug_leads_to_the_published_page() {
        let db = db().await;
        left(&db, PAGE, "p1", &[("about-us", 5)]).await;
        assert_eq!(
            moved_page_slug(&db, "about-us").await.unwrap(),
            None,
            "the page is not published"
        );
        db.execute(
            "INSERT INTO storefront_pages (id, slug, title, body, text, meta_title,
                meta_description, updated_at)
             VALUES ('p1', 'qui-sommes-nous', 'Nous', '', '', '', '', 1)",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            moved_page_slug(&db, "about-us").await.unwrap(),
            Some("qui-sommes-nous".into())
        );
        // A product that left the same slug is another matter.
        assert_eq!(moved_product_slug(&db, "about-us").await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_merchants_list_answers_a_path_with_or_without_its_slash() {
        let db = db().await;
        db.execute(
            "INSERT INTO storefront_redirects (from_path, to_path)
             VALUES ('/products/mug', '/p/mug')",
            (),
        )
        .await
        .unwrap();
        for path in ["/products/mug", "/products/mug/"] {
            assert_eq!(
                redirect_target(&db, path).await.unwrap().as_deref(),
                Some("/p/mug")
            );
        }
        for path in ["/", "/products", "/products/mug/x"] {
            assert_eq!(redirect_target(&db, path).await.unwrap(), None, "{path}");
        }
    }
}
