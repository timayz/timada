//! What a page Search Console lists is in the shop: a category, a product,
//! an article, one of its own pages.
//!
//! A storefront serves a category at its path in the tree
//! (`/jeux-pour-chat/souris`), a product under its category
//! (`…/souris/souris-en-peluche`) or at `/p/{slug}`, an article at
//! `/blog/{slug}` and a page of the shop's at `/{slug}`, each under
//! `/{language}` in a language other than the shop's main one. The address
//! is read back the same way, against the back office's own rows: what is
//! live now. An address that moved since, or one the storefront serves on
//! its own (the cart, a search), is no page known here. A page's or an
//! article's slug in another language is not among the rows, so those are
//! told by the main language's.

use std::collections::HashMap;

use timada_catalog::{category_paths, category_paths_in};

use crate::db::Db;

/// What a page is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landing {
    Home,
    Category(String),
    Product(String),
    /// The list of the articles.
    Blog,
    Article(String),
    /// One of the shop's own pages (legal notice, terms).
    Page(String),
    /// Nothing the back office knows at this address.
    Other,
}

impl Landing {
    /// The kind, in a word, and what the thing is called.
    pub fn label(&self) -> (&'static str, &str) {
        match self {
            Self::Home => ("Home page", ""),
            Self::Category(name) => ("Category", name),
            Self::Product(name) => ("Product", name),
            Self::Blog => ("Blog", ""),
            Self::Article(title) => ("Article", title),
            Self::Page(title) => ("Page", title),
            Self::Other => ("Other", ""),
        }
    }

    /// The kind as a data attribute reads it.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Category(_) => "category",
            Self::Product(_) => "product",
            Self::Blog => "blog",
            Self::Article(_) => "article",
            Self::Page(_) => "page",
            Self::Other => "other",
        }
    }
}

/// The path of an address, as its parts: `https://shop.example/a/b/?x=1`
/// gives `["a", "b"]`. `None` for what is no address.
pub fn parts_of(address: &str) -> Option<Vec<String>> {
    let (_, rest) = address.split_once("://")?;
    let path = rest.find('/').map_or("", |at| &rest[at..]);
    let path = path.split(['?', '#']).next().unwrap_or_default();
    Some(
        path.split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// The path of an address, for a table to show: `/a/b`, `/` for the home
/// page, the address itself when it is none.
pub fn path_of(address: &str) -> String {
    match parts_of(address) {
        Some(parts) => format!("/{}", parts.join("/")),
        None => address.to_owned(),
    }
}

/// The categories of a language: by path, the category's id.
type ByPath = HashMap<String, String>;

/// Tells what the pages of one shop are, reading each kind of row once.
pub struct Landings<'a> {
    db: &'a Db,
    tenant_id: &'a str,
    /// The languages the storefront is read in beside the main one.
    others: &'a [String],
    /// By language (empty for the main one).
    categories: HashMap<String, ByPath>,
    category_names: Option<HashMap<String, String>>,
    /// By language other than the main one: by slug, the product's name
    /// there.
    translated_products: HashMap<String, HashMap<String, String>>,
    pages: Option<HashMap<String, String>>,
    articles: Option<HashMap<String, String>>,
}

impl<'a> Landings<'a> {
    pub fn new(db: &'a Db, tenant_id: &'a str, others: &'a [String]) -> Self {
        Self {
            db,
            tenant_id,
            others,
            categories: HashMap::new(),
            category_names: None,
            translated_products: HashMap::new(),
            pages: None,
            articles: None,
        }
    }

    /// What the page at `address` is.
    pub async fn of(&mut self, address: &str) -> anyhow::Result<Landing> {
        let Some(mut parts) = parts_of(address) else {
            return Ok(Landing::Other);
        };
        let language = match parts.first() {
            Some(first) if self.others.iter().any(|other| other == first) => parts.remove(0),
            _ => String::new(),
        };
        let Some(last) = parts.last().cloned() else {
            return Ok(Landing::Home);
        };
        let path = format!("/{}", parts.join("/"));
        if let Some(name) = self.category(&language, &path).await? {
            return Ok(Landing::Category(name));
        }
        if parts[0] == "blog" {
            return Ok(match parts.len() {
                1 => Landing::Blog,
                2 => self
                    .article(&last)
                    .await?
                    .map_or(Landing::Other, Landing::Article),
                _ => Landing::Other,
            });
        }
        // A product is under a live category, or at `/p/{slug}`.
        let above = format!("/{}", parts[..parts.len() - 1].join("/"));
        let filed = parts.len() >= 2
            && ((parts.len() == 2 && parts[0] == "p")
                || self.category(&language, &above).await?.is_some());
        if filed {
            return Ok(self
                .product(&language, &last)
                .await?
                .map_or(Landing::Other, Landing::Product));
        }
        if parts.len() == 1
            && let Some(title) = self.page(&last).await?
        {
            return Ok(Landing::Page(title));
        }
        Ok(Landing::Other)
    }

    /// The name of the live category at `path` in `language`.
    async fn category(&mut self, language: &str, path: &str) -> anyhow::Result<Option<String>> {
        if !self.categories.contains_key(language) {
            let paths = if language.is_empty() {
                category_paths(self.db, self.tenant_id).await?
            } else {
                category_paths_in(self.db, self.tenant_id, language).await?
            };
            let by_path = paths
                .paths()
                .map(|(id, path)| (path.to_owned(), id.to_owned()))
                .collect();
            self.categories.insert(language.to_owned(), by_path);
        }
        let Some(id) = self.categories[language].get(path).cloned() else {
            return Ok(None);
        };
        if self.category_names.is_none() {
            let rows: Vec<(String, String)> = self
                .db
                .fetch_all(
                    "SELECT id, name FROM catalog_categories
                     WHERE tenant_id = ?1 AND archived = 0 AND draft = 0",
                    (self.tenant_id,),
                )
                .await?;
            self.category_names = Some(rows.into_iter().collect());
        }
        Ok(self
            .category_names
            .as_ref()
            .and_then(|names| names.get(&id).cloned()))
    }

    /// The name of the product that goes by `slug` in `language`.
    async fn product(&mut self, language: &str, slug: &str) -> anyhow::Result<Option<String>> {
        if !language.is_empty() {
            if !self.translated_products.contains_key(language) {
                let rows: Vec<(String, String)> = self
                    .db
                    .fetch_all(
                        "SELECT slug, name FROM catalog_translations
                            INDEXED BY catalog_translations_language
                         WHERE tenant_id = ?1 AND language = ?2 AND kind = 'product'",
                        (self.tenant_id, language),
                    )
                    .await?;
                self.translated_products
                    .insert(language.to_owned(), rows.into_iter().collect());
            }
            if let Some(name) = self.translated_products[language].get(slug) {
                return Ok(Some(name.clone()));
            }
        }
        // A product with no translation goes by its main slug everywhere.
        let name: Option<(String,)> = self
            .db
            .fetch_optional(
                "SELECT name FROM catalog_products INDEXED BY catalog_products_slug
                 WHERE tenant_id = ?1 AND slug = ?2 AND status <> 'archived'",
                (self.tenant_id, slug),
            )
            .await?;
        Ok(name.map(|(name,)| name))
    }

    /// The title of the shop's published page that goes by `slug`.
    async fn page(&mut self, slug: &str) -> anyhow::Result<Option<String>> {
        if self.pages.is_none() {
            let rows: Vec<(String, String)> = self
                .db
                .fetch_all(
                    "SELECT slug, title FROM pages WHERE tenant_id = ?1 AND published = 1",
                    (self.tenant_id,),
                )
                .await?;
            self.pages = Some(rows.into_iter().collect());
        }
        Ok(self
            .pages
            .as_ref()
            .and_then(|pages| pages.get(slug).cloned()))
    }

    /// The title of the published article that goes by `slug`.
    async fn article(&mut self, slug: &str) -> anyhow::Result<Option<String>> {
        if self.articles.is_none() {
            let rows: Vec<(String, String)> = self
                .db
                .fetch_all(
                    "SELECT slug, title FROM articles WHERE tenant_id = ?1 AND published = 1",
                    (self.tenant_id,),
                )
                .await?;
            self.articles = Some(rows.into_iter().collect());
        }
        Ok(self
            .articles
            .as_ref()
            .and_then(|articles| articles.get(slug).cloned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_read_as_the_parts_of_its_path() {
        assert_eq!(
            parts_of("https://shop.example/jeux/souris/?utm=x#top"),
            Some(vec!["jeux".to_owned(), "souris".to_owned()])
        );
        assert_eq!(parts_of("https://shop.example"), Some(Vec::new()));
        assert_eq!(parts_of("https://shop.example/"), Some(Vec::new()));
        assert_eq!(parts_of("https://shop.example?x=1"), Some(Vec::new()));
        assert_eq!(parts_of("shop.example/jeux"), None);
        assert_eq!(path_of("https://shop.example/en/toys"), "/en/toys");
        assert_eq!(path_of("https://shop.example/"), "/");
        assert_eq!(path_of("not an address"), "not an address");
    }

    /// A shop with a parent and a child category, a product filed under
    /// the child and translated, one filed nowhere, an article and a page.
    async fn shop() -> Db {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        let db = Db::from(turso);
        for (id, slug, name, parent, archived, draft) in [
            ("c1", "jeux", "Jeux pour chat", None, 0, 0),
            ("c2", "souris", "Souris", Some("c1"), 0, 0),
            ("c3", "anciens", "Anciens", None, 1, 0),
            ("c4", "bientot", "Bientôt", None, 0, 1),
        ] {
            db.execute(
                "INSERT INTO catalog_categories (tenant_id, id, slug, name, description,
                    parent_id, position, archived, in_menu, created_by, created_at, updated_at,
                    draft)
                 VALUES ('shop', ?1, ?2, ?3, '', ?4, 0, ?5, 1, 'alice', 0, 0, ?6)",
                (id, slug, name, parent, archived, draft),
            )
            .await
            .unwrap();
        }
        for (id, slug, name, status) in [
            ("p1", "souris-en-peluche", "Souris en peluche", "published"),
            ("p2", "plumeau", "Plumeau", "published"),
            ("p3", "retire", "Retiré", "archived"),
        ] {
            db.execute(
                "INSERT INTO catalog_products (tenant_id, id, slug, name, status, created_by,
                    created_at, updated_at, image_count, variant_count, tax_category,
                    slug_search, name_search)
                 VALUES ('shop', ?1, ?2, ?3, ?4, 'alice', 0, 0, 0, 0, 'standard', ?2, ?3)",
                (id, slug, name, status),
            )
            .await
            .unwrap();
        }
        for (kind, subject, name, slug) in [
            ("category", "c1", "Cat toys", "toys"),
            ("category", "c2", "Mice", "mice"),
            ("product", "p1", "Plush mouse", "plush-mouse"),
        ] {
            db.execute(
                "INSERT INTO catalog_translations (tenant_id, kind, subject_id, language, name,
                    slug, description)
                 VALUES ('shop', ?1, ?2, 'en', ?3, ?4, '')",
                (kind, subject, name, slug),
            )
            .await
            .unwrap();
        }
        db.execute(
            "INSERT INTO pages (tenant_id, id, slug, title, published, created_at, updated_at)
             VALUES ('shop', 'g1', 'mentions-legales', 'Mentions légales', 1, 0, 0),
                    ('shop', 'g2', 'brouillon', 'Brouillon', 0, 0, 0)",
            (),
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO articles (tenant_id, id, slug, title, published, published_at,
                listed_at, updated_at)
             VALUES ('shop', 'a1', 'choisir-un-arbre', 'Choisir un arbre à chat', 1, 0, 0, 0)",
            (),
        )
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn a_page_is_told_for_what_it_is_in_the_shop() {
        let db = shop().await;
        let others = vec!["en".to_owned()];
        let mut landings = Landings::new(&db, "shop", &others);
        let category = |name: &str| Landing::Category(name.to_owned());
        let product = |name: &str| Landing::Product(name.to_owned());
        for (path, expected) in [
            ("", Landing::Home),
            ("/", Landing::Home),
            ("/?utm_source=x", Landing::Home),
            ("/jeux", category("Jeux pour chat")),
            ("/jeux/souris", category("Souris")),
            ("/jeux/souris/", category("Souris")),
            (
                "/jeux/souris/souris-en-peluche",
                product("Souris en peluche"),
            ),
            ("/p/plumeau", product("Plumeau")),
            ("/blog", Landing::Blog),
            (
                "/blog/choisir-un-arbre",
                Landing::Article("Choisir un arbre à chat".to_owned()),
            ),
            (
                "/mentions-legales",
                Landing::Page("Mentions légales".to_owned()),
            ),
            // In the other language, by the slugs it goes by there.
            ("/en", Landing::Home),
            ("/en/toys", category("Jeux pour chat")),
            ("/en/toys/mice", category("Souris")),
            ("/en/toys/mice/plush-mouse", product("Plush mouse")),
            // No translation: the main slug stands.
            ("/en/p/plumeau", product("Plumeau")),
            (
                "/en/blog/choisir-un-arbre",
                Landing::Article("Choisir un arbre à chat".to_owned()),
            ),
            // Nothing live at these.
            ("/anciens", Landing::Other),
            ("/bientot", Landing::Other),
            ("/brouillon", Landing::Other),
            ("/jeux/inconnu", Landing::Other),
            ("/p/retire", Landing::Other),
            ("/blog/inconnu", Landing::Other),
            ("/panier", Landing::Other),
            ("/toys", Landing::Other),
            // The main language's path under another's prefix.
            ("/en/jeux", Landing::Other),
        ] {
            let address = format!("https://shop.example{path}");
            assert_eq!(landings.of(&address).await.unwrap(), expected, "{path}");
        }
        assert_eq!(landings.of("no address").await.unwrap(), Landing::Other);
        // Another shop's rows are not this one's.
        let mut theirs = Landings::new(&db, "other", &[]);
        assert_eq!(
            theirs.of("https://shop.example/jeux").await.unwrap(),
            Landing::Other
        );
        // Without the language, `/en` is a path like another.
        assert_eq!(
            theirs.of("https://shop.example/en").await.unwrap(),
            Landing::Other
        );
    }
}
