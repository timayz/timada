//! What a search engine is told to read: the addresses of a sitemap, read
//! off the local copy so they answer when the store is away, and the XML
//! and `robots.txt` they are written as. Which address a product, a
//! category, a page or an article has is the host's: the library gives the slugs and
//! writes the files.

use timada_core::civil::format_date;
use timada_core::db::{Db, FromRow};
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::store;
use crate::read_model::WHOLE_SHOP;

/// How many products one sitemap file lists. The protocol allows 50,000; a
/// fifth of that keeps a file around a megabyte and its read short.
pub const SITEMAP_PRODUCTS: u32 = 10_000;

/// Something a sitemap lists: its slug, and when it last changed (unix
/// seconds).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SitemapEntry {
    pub slug: String,
    pub updated_at: i64,
}

/// How many files the published products fill, [`SITEMAP_PRODUCTS`] each:
/// 0 for a shop with nothing on sale.
pub async fn published_sitemap_parts(db: &Db) -> anyhow::Result<u32> {
    let count: Option<(i64,)> = db
        .fetch_optional("SELECT products FROM storefront_shop", ())
        .await?;
    let count = count.map_or(0, |(count,)| count.max(0)) as u64;
    Ok(count.div_ceil(u64::from(SITEMAP_PRODUCTS)) as u32)
}

/// The published products of file `part` (from 1), oldest first: a product
/// stays in its file as others are added after it. Read off the whole
/// shop's listing in the order of its key; the files before it are skipped
/// over that key, which only a search engine pays, a few times a day.
pub async fn published_sitemap_products(db: &Db, part: u32) -> anyhow::Result<Vec<SitemapEntry>> {
    let Some(before) = part.checked_sub(1) else {
        return Ok(Vec::new());
    };
    db.fetch_all(
        "SELECT p.slug, p.updated_at
         FROM storefront_category_products cp
         JOIN storefront_products p ON p.id = cp.product_id
         WHERE cp.category_id = ?1
         ORDER BY cp.category_id, cp.product_id LIMIT ?2 OFFSET ?3",
        (
            WHOLE_SHOP,
            i64::from(SITEMAP_PRODUCTS),
            i64::from(before) * i64::from(SITEMAP_PRODUCTS),
        ),
    )
    .await
}

/// The live categories, in no particular order.
pub async fn published_sitemap_categories(db: &Db) -> anyhow::Result<Vec<SitemapEntry>> {
    db.fetch_all(
        "SELECT slug, updated_at FROM storefront_categories WHERE archived = 0 ORDER BY id",
        (),
    )
    .await
}

/// The published pages of the shop. The host lists those it has a route
/// for.
pub async fn published_sitemap_pages(db: &Db) -> anyhow::Result<Vec<SitemapEntry>> {
    db.fetch_all(
        "SELECT slug, max(updated_at) AS updated_at FROM storefront_pages GROUP BY slug ORDER BY slug",
        (),
    )
    .await
}

/// How many articles a sitemap lists: the most recent ones, should a blog
/// ever hold more.
pub const SITEMAP_ARTICLES: u32 = 10_000;

/// The published articles of the blog, the most recent first.
pub async fn published_sitemap_articles(db: &Db) -> anyhow::Result<Vec<SitemapEntry>> {
    db.fetch_all(
        "SELECT slug, updated_at FROM storefront_articles
         INDEXED BY storefront_articles_published
         ORDER BY published_at DESC, id DESC LIMIT ?1",
        (i64::from(SITEMAP_ARTICLES),),
    )
    .await
}

/// The slug a row of `rows` (named `alias`) goes by in the language bound
/// as `?{param}`: its translation's, read through its primary key, or its
/// own.
fn slug_in(translations: &str, kind: &str, alias: &str, param: u8) -> String {
    format!(
        "COALESCE((SELECT t.slug FROM {translations} t
                   WHERE t.kind = '{kind}' AND t.subject_id = {alias}.id AND t.language = ?{param}),
                  {alias}.slug)"
    )
}

/// [`published_sitemap_products`] at the slugs `language` reads them at.
pub async fn published_sitemap_products_in(
    db: &Db,
    part: u32,
    language: &str,
) -> anyhow::Result<Vec<SitemapEntry>> {
    let Some(before) = part.checked_sub(1) else {
        return Ok(Vec::new());
    };
    db.fetch_all(
        &format!(
            "SELECT {} AS slug, p.updated_at
             FROM storefront_category_products cp
             JOIN storefront_products p ON p.id = cp.product_id
             WHERE cp.category_id = ?1
             ORDER BY cp.category_id, cp.product_id LIMIT ?2 OFFSET ?3",
            slug_in("storefront_translations", "product", "p", 4)
        ),
        (
            WHOLE_SHOP,
            i64::from(SITEMAP_PRODUCTS),
            i64::from(before) * i64::from(SITEMAP_PRODUCTS),
            language,
        ),
    )
    .await
}

/// [`published_sitemap_categories`] at the slugs `language` reads them at.
pub async fn published_sitemap_categories_in(
    db: &Db,
    language: &str,
) -> anyhow::Result<Vec<SitemapEntry>> {
    db.fetch_all(
        &format!(
            "SELECT {} AS slug, c.updated_at FROM storefront_categories c
             WHERE c.archived = 0 ORDER BY c.id",
            slug_in("storefront_translations", "category", "c", 1)
        ),
        (language,),
    )
    .await
}

/// [`published_sitemap_articles`] at the slugs `language` reads them at.
pub async fn published_sitemap_articles_in(
    db: &Db,
    language: &str,
) -> anyhow::Result<Vec<SitemapEntry>> {
    db.fetch_all(
        &format!(
            "SELECT {} AS slug, a.updated_at FROM storefront_articles a
             INDEXED BY storefront_articles_published
             ORDER BY a.published_at DESC, a.id DESC LIMIT ?2",
            slug_in("storefront_text_translations", "article", "a", 1)
        ),
        (language, i64::from(SITEMAP_ARTICLES)),
    )
    .await
}

/// How many product files the sitemap has ([`sitemap_products`] reads one).
pub async fn sitemap_parts(cx: &Cx) -> Result<u32> {
    published_sitemap_parts(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// The published products of sitemap file `part`, from 1 to
/// [`sitemap_parts`]; empty past the last.
///
/// A request read in another language ([`read_in`](crate::read_in)) gets
/// the slugs of that language, here and from the two below.
pub async fn sitemap_products(cx: &Cx, part: u32) -> Result<Vec<SitemapEntry>> {
    let db = store(cx).db();
    match crate::read_in(cx) {
        Some(language) => published_sitemap_products_in(db, part, language).await,
        None => published_sitemap_products(db, part).await,
    }
    .map_err(Error::from_anyhow)
}

/// The live categories, for the sitemap.
pub async fn sitemap_categories(cx: &Cx) -> Result<Vec<SitemapEntry>> {
    let db = store(cx).db();
    match crate::read_in(cx) {
        Some(language) => published_sitemap_categories_in(db, language).await,
        None => published_sitemap_categories(db).await,
    }
    .map_err(Error::from_anyhow)
}

/// The shop's published pages, for the sitemap.
pub async fn sitemap_pages(cx: &Cx) -> Result<Vec<SitemapEntry>> {
    published_sitemap_pages(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// The blog's published articles, for the sitemap.
pub async fn sitemap_articles(cx: &Cx) -> Result<Vec<SitemapEntry>> {
    let db = store(cx).db();
    match crate::read_in(cx) {
        Some(language) => published_sitemap_articles_in(db, language).await,
        None => published_sitemap_articles(db).await,
    }
    .map_err(Error::from_anyhow)
}

/// The address shoppers reach `path` at: under
/// [`StoreConfig::public_url`](crate::StoreConfig::public_url), or the path
/// alone when the host gave none. A sitemap and `robots.txt` need the whole
/// address.
pub fn absolute_url(cx: &Cx, path: &str) -> String {
    format!("{}{path}", store(cx).public_url().unwrap_or_default())
}

/// The media type a sitemap is served as.
pub const SITEMAP_CONTENT_TYPE: &str = "application/xml; charset=utf-8";

/// A sitemap file being written: `<urlset>`, one `<url>` per address.
#[derive(Debug)]
pub struct Sitemap {
    xml: String,
}

impl Default for Sitemap {
    fn default() -> Self {
        Self::new()
    }
}

impl Sitemap {
    pub fn new() -> Self {
        Self {
            xml: concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n"
            )
            .to_owned(),
        }
    }

    /// Lists `loc`, a whole address, changed at `updated_at` (unix seconds)
    /// when the page knows.
    #[must_use]
    pub fn url(mut self, loc: &str, updated_at: Option<i64>) -> Self {
        push_entry(&mut self.xml, "url", loc, updated_at);
        self
    }

    /// The file.
    pub fn xml(mut self) -> String {
        self.xml.push_str("</urlset>\n");
        self.xml
    }
}

/// A sitemap index being written: `<sitemapindex>`, one `<sitemap>` per
/// file.
#[derive(Debug)]
pub struct SitemapIndex {
    xml: String,
}

impl Default for SitemapIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SitemapIndex {
    pub fn new() -> Self {
        Self {
            xml: concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n"
            )
            .to_owned(),
        }
    }

    /// Lists the sitemap file at `loc`, a whole address.
    #[must_use]
    pub fn sitemap(mut self, loc: &str) -> Self {
        push_entry(&mut self.xml, "sitemap", loc, None);
        self
    }

    /// The file.
    pub fn xml(mut self) -> String {
        self.xml.push_str("</sitemapindex>\n");
        self.xml
    }
}

fn push_entry(xml: &mut String, tag: &str, loc: &str, updated_at: Option<i64>) {
    xml.push_str(&format!("<{tag}><loc>{}</loc>", escape(loc)));
    if let Some(at) = updated_at.and_then(|at| u64::try_from(at).ok()) {
        xml.push_str(&format!("<lastmod>{}</lastmod>", format_date(at)));
    }
    xml.push_str(&format!("</{tag}>\n"));
}

/// `text` as XML character data.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// A `robots.txt` for every crawler: each of `disallow` kept out, the rest
/// open, and the sitemap named by its whole address.
pub fn robots_txt(sitemap: &str, disallow: &[&str]) -> String {
    let mut txt = String::from("User-agent: *\n");
    for path in disallow {
        txt.push_str(&format!("Disallow: {path}\n"));
    }
    if disallow.is_empty() {
        txt.push_str("Disallow:\n");
    }
    txt.push_str(&format!("\nSitemap: {sitemap}\n"));
    txt
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

    async fn product(db: &Db, id: &str, status: &str, updated_at: i64) {
        db.execute(
            "INSERT INTO storefront_products (id, slug, name, description, status, updated_at,
                images_json, options_json, variants_json, attributes_json, slug_search,
                name_search)
             VALUES (?1, ?1, ?1, '', ?2, ?3, '[]', '[]', '[]', '[]', ?1, ?1)",
            (id, status, updated_at),
        )
        .await
        .unwrap();
        if status == "published" {
            db.execute(
                "INSERT INTO storefront_category_products (category_id, product_id, updated_at,
                    review_count, rating_x100)
                 VALUES ('', ?1, ?2, 0, 0)",
                (id, updated_at),
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn the_products_fill_files_oldest_first_and_drafts_stay_out() {
        let db = db().await;
        assert_eq!(published_sitemap_parts(&db).await.unwrap(), 0);
        product(&db, "b-mug", "published", 20).await;
        product(&db, "a-mat", "published", 10).await;
        product(&db, "c-draft", "draft", 30).await;
        db.execute(
            "INSERT INTO storefront_shop (id, products) VALUES (1, 2)",
            (),
        )
        .await
        .unwrap();
        assert_eq!(published_sitemap_parts(&db).await.unwrap(), 1);
        let entry = |slug: &str, updated_at| SitemapEntry {
            slug: slug.to_owned(),
            updated_at,
        };
        assert_eq!(
            published_sitemap_products(&db, 1).await.unwrap(),
            [entry("a-mat", 10), entry("b-mug", 20)]
        );
        assert_eq!(published_sitemap_products(&db, 2).await.unwrap(), []);
        assert_eq!(published_sitemap_products(&db, 0).await.unwrap(), []);

        // One more product than a file holds makes a second file.
        db.execute(
            "UPDATE storefront_shop SET products = ?1",
            (i64::from(SITEMAP_PRODUCTS) + 1,),
        )
        .await
        .unwrap();
        assert_eq!(published_sitemap_parts(&db).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn a_products_file_walks_the_listing_in_order() {
        let db = db().await;
        let plan: Vec<String> = db
            .fetch_all::<(i64, i64, i64, String)>(
                "EXPLAIN QUERY PLAN SELECT p.slug, p.updated_at
                 FROM storefront_category_products cp
                 JOIN storefront_products p ON p.id = cp.product_id
                 WHERE cp.category_id = ?1
                 ORDER BY cp.category_id, cp.product_id LIMIT ?2 OFFSET ?3",
                ("", 10, 0),
            )
            .await
            .unwrap()
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect();
        assert!(
            plan.iter().all(|step| step.starts_with("SEARCH")),
            "{plan:#?}"
        );
    }

    #[tokio::test]
    async fn live_categories_and_published_pages_are_listed() {
        let db = db().await;
        for (id, archived) in [("mats", false), ("old", true)] {
            db.execute(
                "INSERT INTO storefront_categories (id, slug, name, description, body, position,
                    archived, in_menu, updated_at)
                 VALUES (?1, ?1, ?1, '', '', 0, ?2, 1, 5)",
                (id, archived),
            )
            .await
            .unwrap();
        }
        db.execute(
            "INSERT INTO storefront_pages (id, slug, title, body, text, meta_title,
                meta_description, updated_at)
             VALUES ('p1', 'legal', 'Legal', '', '', '', '', 7)",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            published_sitemap_categories(&db).await.unwrap(),
            [SitemapEntry {
                slug: "mats".into(),
                updated_at: 5
            }]
        );
        assert_eq!(
            published_sitemap_pages(&db).await.unwrap(),
            [SitemapEntry {
                slug: "legal".into(),
                updated_at: 7
            }]
        );
    }

    #[test]
    fn a_sitemap_escapes_its_addresses_and_dates_them_by_day() {
        let xml = Sitemap::new()
            .url("https://shop.example/", None)
            .url("https://shop.example/p/mug?a=1&b=<2>", Some(1_709_164_800))
            .xml();
        assert_eq!(
            xml,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n\
             <url><loc>https://shop.example/</loc></url>\n\
             <url><loc>https://shop.example/p/mug?a=1&amp;b=&lt;2&gt;</loc>\
             <lastmod>2024-02-29</lastmod></url>\n\
             </urlset>\n"
        );
        let index = SitemapIndex::new()
            .sitemap("https://shop.example/sitemap-pages.xml")
            .xml();
        assert!(index.contains(
            "<sitemap><loc>https://shop.example/sitemap-pages.xml</loc></sitemap>\n</sitemapindex>"
        ));
    }

    #[test]
    fn robots_keep_out_what_they_are_told_and_name_the_sitemap() {
        assert_eq!(
            robots_txt("https://shop.example/sitemap.xml", &["/cart", "/*?*sort="]),
            "User-agent: *\nDisallow: /cart\nDisallow: /*?*sort=\n\n\
             Sitemap: https://shop.example/sitemap.xml\n"
        );
        assert_eq!(
            robots_txt("/sitemap.xml", &[]),
            "User-agent: *\nDisallow:\n\nSitemap: /sitemap.xml\n"
        );
    }
}
