//! Where an old address went. A product, a page or an article renamed in
//! the back office leaves its slug; the copy remembers the slugs each one left
//! (`storefront_former_slugs`), and an archived product's last one, so the
//! old address can answer with a permanent redirect instead of a 404:
//! [`moved_product`], [`moved_page`] and [`moved_article`]. A live slug always wins: the host
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
/// The `kind` of a blog article's rows.
pub(crate) const ARTICLE: &str = "article";
/// The `kind` of a category's rows: a category keeps its main slug for
/// life, so these are the slugs its translations left.
pub(crate) const CATEGORY: &str = "category";
/// The `language` of the slugs a thing left in the shop's main language.
pub(crate) const MAIN: &str = "";

/// Makes the former slugs of one thing in one language ([`MAIN`] for the
/// main one) in the copy what `wanted` says, writing only when they
/// differ: most events of a product move none.
pub(crate) async fn write_former_slugs(
    tx: &Connection,
    kind: &str,
    language: &str,
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
         WHERE kind = ?1 AND id = ?2 AND language = ?3 ORDER BY slug, left_at",
        (kind, id, language),
    )
    .await?;
    if were == wanted {
        return Ok(());
    }
    execute(
        tx,
        "DELETE FROM storefront_former_slugs WHERE kind = ?1 AND id = ?2 AND language = ?3",
        (kind, id, language),
    )
    .await?;
    for (slug, left_at) in &wanted {
        execute(
            tx,
            "INSERT INTO storefront_former_slugs (kind, language, slug, id, left_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (kind, language, slug.as_str(), id, *left_at),
        )
        .await?;
    }
    Ok(())
}

/// Forgets the former slugs of a thing that is gone, in every language.
pub(crate) async fn forget_former_slugs(
    tx: &Connection,
    kind: &str,
    id: &str,
) -> anyhow::Result<()> {
    execute(
        tx,
        "DELETE FROM storefront_former_slugs WHERE kind = ?1 AND id = ?2",
        (kind, id),
    )
    .await?;
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
    let left = products_that_left(db, MAIN, slug).await?;
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

/// The products that left `slug` in `language`, the latest leaver first:
/// each one's id, main slug and status.
async fn products_that_left(
    db: &Db,
    language: &str,
    slug: &str,
) -> anyhow::Result<Vec<(String, String, String)>> {
    db.fetch_all(
        "SELECT p.id, p.slug, p.status
         FROM storefront_former_slugs f
         JOIN storefront_products p ON p.id = f.id
         WHERE f.kind = ?1 AND f.language = ?2 AND f.slug = ?3
         ORDER BY f.left_at DESC, f.id",
        (PRODUCT, language, slug),
    )
    .await
}

/// The slug the thing `id` of `kind` goes by in `language`: its
/// translation's, or its main one where it has none (a dropped
/// translation's old address leads there).
async fn slug_in(db: &Db, kind: &str, id: &str, language: &str) -> anyhow::Result<Option<String>> {
    Ok(crate::read_in::slugs_of(db, kind, id)
        .await?
        .map(|slugs| slugs.in_language(language).to_owned()))
}

/// [`moved_product_slug`] for an address read in `language`, one of the
/// shop's other languages: the slugs a translation left there first, then
/// the ones left in the main language, which are a product's address there
/// while it has no translation. What is returned is the slug in that
/// language; `None` too when it is `slug` itself, which the page already
/// answered for.
pub async fn moved_product_slug_in(
    db: &Db,
    language: &str,
    slug: &str,
) -> anyhow::Result<Option<MovedProduct>> {
    let mut left = products_that_left(db, language, slug).await?;
    left.extend(products_that_left(db, MAIN, slug).await?);
    if let Some((id, _, _)) = left.iter().find(|(_, _, status)| status == "published") {
        return Ok(slug_in(db, PRODUCT, id, language)
            .await?
            .filter(|now| now != slug)
            .map(MovedProduct::Product));
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
            let slug = slug_in(db, CATEGORY, &category.id, language)
                .await?
                .unwrap_or(category.slug);
            return Ok(Some(MovedProduct::Category(slug)));
        }
    }
    Ok(None)
}

/// The slug, in `language`, of the live category whose translation there
/// last went by `slug`, if any. A category keeps its main slug for life:
/// only a translation's address moves.
pub async fn moved_category_slug_in(
    db: &Db,
    language: &str,
    slug: &str,
) -> anyhow::Result<Option<String>> {
    let left: Option<(String,)> = db
        .fetch_optional(
            "SELECT c.id
             FROM storefront_former_slugs f
             JOIN storefront_categories c ON c.id = f.id
             WHERE f.kind = ?1 AND f.language = ?2 AND f.slug = ?3 AND c.archived = 0
             ORDER BY f.left_at DESC, f.id LIMIT 1",
            (CATEGORY, language, slug),
        )
        .await?;
    let Some((id,)) = left else {
        return Ok(None);
    };
    Ok(slug_in(db, CATEGORY, &id, language)
        .await?
        .filter(|now| now != slug))
}

/// The slug of the published page that last went by `slug`, if any.
pub async fn moved_page_slug(db: &Db, slug: &str) -> anyhow::Result<Option<String>> {
    let now: Option<(String,)> = db
        .fetch_optional(
            "SELECT p.slug
             FROM storefront_former_slugs f
             JOIN storefront_pages p ON p.id = f.id
             WHERE f.kind = ?1 AND f.language = ?2 AND f.slug = ?3
             ORDER BY f.left_at DESC, f.id LIMIT 1",
            (PAGE, MAIN, slug),
        )
        .await?;
    Ok(now.map(|(slug,)| slug))
}

/// The slug of the published article that last went by `slug`, if any.
pub async fn moved_article_slug(db: &Db, slug: &str) -> anyhow::Result<Option<String>> {
    Ok(article_that_left(db, MAIN, slug)
        .await?
        .map(|(_, slug)| slug))
}

/// The published article that last left `slug` in `language`: its id and
/// its main slug.
async fn article_that_left(
    db: &Db,
    language: &str,
    slug: &str,
) -> anyhow::Result<Option<(String, String)>> {
    db.fetch_optional(
        "SELECT a.id, a.slug
         FROM storefront_former_slugs f
         JOIN storefront_articles a ON a.id = f.id
         WHERE f.kind = ?1 AND f.language = ?2 AND f.slug = ?3
         ORDER BY f.left_at DESC, f.id LIMIT 1",
        (ARTICLE, language, slug),
    )
    .await
}

/// [`moved_article_slug`] for an address read in `language`: the slugs a
/// translation left there first, then the ones left in the main language.
/// What is returned is the slug in that language.
pub async fn moved_article_slug_in(
    db: &Db,
    language: &str,
    slug: &str,
) -> anyhow::Result<Option<String>> {
    let left = match article_that_left(db, language, slug).await? {
        Some(left) => Some(left),
        None => article_that_left(db, MAIN, slug).await?,
    };
    let Some((id, _)) = left else {
        return Ok(None);
    };
    Ok(slug_in(db, ARTICLE, &id, language)
        .await?
        .filter(|now| now != slug))
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

/// Where the product that went by `slug` is now, in the language the
/// request is read in; see [`moved_product_slug`] and
/// [`moved_product_slug_in`]. For a product page that found nothing.
pub async fn moved_product(cx: &Cx, slug: &str) -> Result<Option<MovedProduct>> {
    let db = store(cx).db();
    match crate::read_in(cx) {
        Some(language) => moved_product_slug_in(db, language, slug).await,
        None => moved_product_slug(db, slug).await,
    }
    .map_err(Error::from_anyhow)
}

/// The slug of the live category that went by `slug` in the language the
/// request is read in; see [`moved_category_slug_in`]. Always `None` in
/// the main language, where a category never moves.
pub async fn moved_category(cx: &Cx, slug: &str) -> Result<Option<String>> {
    match crate::read_in(cx) {
        Some(language) => moved_category_slug_in(store(cx).db(), language, slug)
            .await
            .map_err(Error::from_anyhow),
        None => Ok(None),
    }
}

/// The slug of the published page that went by `slug`; see
/// [`moved_page_slug`].
pub async fn moved_page(cx: &Cx, slug: &str) -> Result<Option<String>> {
    moved_page_slug(store(cx).db(), slug)
        .await
        .map_err(Error::from_anyhow)
}

/// The slug of the published article that went by `slug`, in the language
/// the request is read in; see [`moved_article_slug`] and
/// [`moved_article_slug_in`].
pub async fn moved_article(cx: &Cx, slug: &str) -> Result<Option<String>> {
    let db = store(cx).db();
    match crate::read_in(cx) {
        Some(language) => moved_article_slug_in(db, language, slug).await,
        None => moved_article_slug(db, slug).await,
    }
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
            Box::pin(async move { write_former_slugs(tx, kind, MAIN, id, former).await })
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
