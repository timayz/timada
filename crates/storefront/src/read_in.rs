//! A request read in one of the shop's other languages: what the library's
//! readers show is the translation where there is one (see
//! [`translations`](crate::translation_of)), part by part, and the main
//! wording for the rest.
//!
//! The host says which language a request is read in by carrying a
//! [`ReadIn`] in its context (the storefront does on the way from
//! `/en/…` to the page); a request without one is read in the main
//! language, straight from the rows.

use std::collections::HashMap;

use timada_catalog::word_key;
use timada_core::db::Db;
use timada_pages::{render_article, render_markdown};
use topcoat_core::context::{Cx, try_context};

use crate::articles::{ArticleCard, PublicArticle};
use crate::cart::CartView;
use crate::pages::{DESCRIPTION_CHARS, PublicPage, excerpt};
use crate::query::{Category, CategoryHub, CategoryNode, Product, Seo, published_products_by_ids};
use crate::shoppers::request_language;
use crate::translations::{
    TEXT_TRANSLATIONS_KEY, TRANSLATIONS_KEY, TextKind, TextTranslation, Translation,
    TranslationKind, text_translation_of, translation_of,
};
use crate::words::published_words;

/// The language, other than the shop's main one, a request is read in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadIn(String);

impl ReadIn {
    pub fn new(language: impl Into<String>) -> Self {
        ReadIn(language.into())
    }
}

/// The language the request is read in when it is not the shop's main one.
pub fn read_in(cx: &Cx) -> Option<&str> {
    try_context::<ReadIn>(cx).map(|language| language.0.as_str())
}

/// The language of the address a request came at, in a shop read in
/// several: the main one at an address without a prefix as much as another
/// under its own. The host carries it in the request's context beside
/// [`ReadIn`] (the storefront does for every page of such a shop); a shop
/// read in one language has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressLanguage(String);

impl AddressLanguage {
    pub fn new(language: impl Into<String>) -> Self {
        AddressLanguage(language.into())
    }
}

/// The language the shop writes to a shopper in, for what they ask from
/// this request: the order's mails, the sign-in code, the newsletter's
/// confirmation, the answer to a message. In a shop read in several
/// languages it is the one of the address ([`AddressLanguage`]), what the
/// shopper read the page in; a shop read in one goes by the browser's
/// ([`request_language`]).
pub fn shopper_language(cx: &Cx) -> Option<String> {
    match try_context::<AddressLanguage>(cx) {
        Some(language) => Some(language.0.clone()),
        None => request_language(cx),
    }
}

/// `theirs` unless it is empty.
fn said(theirs: String, main: &mut String) {
    if !theirs.is_empty() {
        *main = theirs;
    }
}

/// Words a product as `translation` does.
fn word_product(product: &mut Product, translation: Translation) {
    product.name = translation.name;
    product.slug = translation.slug;
    said(translation.description, &mut product.description);
    for image in &mut product.images {
        if let Some(alt) = translation.alts.get(&image.id) {
            image.alt = alt.clone();
        }
    }
}

/// Words each product in `language`, where it has a translation: its name,
/// its slug there and its description.
pub(crate) async fn products_in(
    db: &Db,
    language: &str,
    products: &mut [Product],
) -> anyhow::Result<()> {
    // Each through the primary key: a page of a listing at most.
    for product in products.iter_mut() {
        if let Some(translation) =
            translation_of(db, TranslationKind::Product, &product.id, language).await?
        {
            word_product(product, translation);
        }
    }
    product_words_in(db, language, products).await
}

/// Words what `products` share with the rest of the shop in `language`:
/// the names of their options and the values these take, which become the
/// labels a shopper reads ([`ProductOption::display_name`],
/// [`ProductOption::display_value`]), and the facts of their tables. The
/// words of them all are read at once; one the shop has not worded there
/// stays as the main language writes it.
///
/// [`ProductOption::display_name`]: crate::ProductOption::display_name
/// [`ProductOption::display_value`]: crate::ProductOption::display_value
async fn product_words_in(db: &Db, language: &str, products: &mut [Product]) -> anyhow::Result<()> {
    let mut terms: Vec<&str> = Vec::new();
    for product in products.iter() {
        for option in &product.options {
            terms.push(option.display_name());
            terms.extend(
                option
                    .values
                    .iter()
                    .map(|value| option.display_value(value)),
            );
        }
        for fact in &product.attributes {
            terms.extend([fact.name.as_str(), fact.value.as_str()]);
        }
    }
    let words = published_words(db, language, terms);
    let words = words.await?;
    if words.is_empty() {
        return Ok(());
    }
    let word = |term: &str| words.get(&word_key(term)).cloned();
    for product in products {
        for option in &mut product.options {
            if let Some(name) = word(option.display_name()) {
                option.label = Some(name);
            }
            for value in &option.values {
                if let Some(wording) = word(option.display_value(value)) {
                    option.value_labels.insert(value.clone(), wording);
                }
            }
        }
        for fact in &mut product.attributes {
            if let Some(name) = word(&fact.name) {
                fact.name = name;
            }
            if let Some(value) = word(&fact.value) {
                fact.value = value;
            }
        }
    }
    Ok(())
}

/// Words a cart's lines and its offers in `language`: the name of each
/// product that has a translation, the slug its page goes by there, the
/// values of the variant taken as the shop words them there, and the names
/// of the ways it ships. The cart
/// itself keeps no name: the dashboard prices it from the catalog, in the
/// main language.
pub(crate) async fn cart_in(db: &Db, language: &str, cart: &mut CartView) -> anyhow::Result<()> {
    let mut worded: HashMap<String, Option<Translation>> = HashMap::new();
    let ids = cart
        .lines
        .iter()
        .map(|line| &line.product_id)
        .chain(cart.offers.iter().map(|offer| &offer.product_id));
    for id in ids {
        if !worded.contains_key(id) {
            let translation = translation_of(db, TranslationKind::Product, id, language).await?;
            worded.insert(id.clone(), translation);
        }
    }
    for line in &mut cart.lines {
        if let Some(Some(translation)) = worded.get(&line.product_id) {
            line.name = translation.name.clone();
            // Only a product still on sale here has a page to link to.
            if line.slug.is_some() {
                line.slug = Some(translation.slug.clone());
            }
        }
    }
    for offer in &mut cart.offers {
        if let Some(Some(translation)) = worded.get(&offer.product_id) {
            offer.name = translation.name.clone();
            if offer.slug.is_some() {
                offer.slug = Some(translation.slug.clone());
            }
        }
    }
    // The ways it ships, as the language calls them.
    let names: Vec<(String, String)> = db
        .fetch_all(
            "SELECT method_id, name FROM storefront_shipping_names WHERE language = ?1",
            (language,),
        )
        .await?;
    for option in &mut cart.shipping_options {
        if let Some((_, name)) = names.iter().find(|(id, _)| *id == option.id) {
            option.name = name.clone();
        }
    }
    // A variant's values, from the products still on sale here: one that
    // is not keeps the label the cart came with.
    let varied: Vec<&str> = cart
        .lines
        .iter()
        .filter(|line| line.variant_label.is_some())
        .map(|line| line.product_id.as_str())
        .chain(
            cart.offers
                .iter()
                .filter(|offer| offer.variant_label.is_some())
                .map(|offer| offer.product_id.as_str()),
        )
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut products = published_products_by_ids(db, &varied).await?;
    product_words_in(db, language, &mut products).await?;
    let label = |product_id: &str, variant_id: Option<&str>| {
        let product = products.iter().find(|product| product.id == product_id)?;
        let variant = product
            .variants
            .iter()
            .find(|variant| Some(variant.id.as_str()) == variant_id)?;
        Some(product.variant_label(variant)).filter(|label| !label.is_empty())
    };
    for line in &mut cart.lines {
        if line.variant_label.is_some()
            && let Some(label) = label(&line.product_id, line.variant_id.as_deref())
        {
            line.variant_label = Some(label);
        }
    }
    for offer in &mut cart.offers {
        if offer.variant_label.is_some()
            && let Some(label) = label(&offer.product_id, offer.variant_id.as_deref())
        {
            offer.variant_label = Some(label);
        }
    }
    Ok(())
}

/// Words a category in `language`, where it has a translation.
pub(crate) async fn category_in(
    db: &Db,
    language: &str,
    category: &mut Category,
) -> anyhow::Result<()> {
    if let Some(translation) =
        translation_of(db, TranslationKind::Category, &category.id, language).await?
    {
        category.name = translation.name;
        category.slug = translation.slug;
        said(translation.description, &mut category.description);
    }
    Ok(())
}

/// Words the whole tree in `language`: every category's translation read
/// at once, as the tree itself is.
pub(crate) async fn categories_in(
    db: &Db,
    language: &str,
    nodes: &mut [CategoryNode],
) -> anyhow::Result<()> {
    let rows: Vec<(String, String, String, String)> = db
        .fetch_all(
            "SELECT subject_id, name, slug, description FROM storefront_translations
             WHERE kind = 'category' AND language = ?1",
            (language,),
        )
        .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut worded: HashMap<String, (String, String, String)> = rows
        .into_iter()
        .map(|(id, name, slug, description)| (id, (name, slug, description)))
        .collect();
    fn walk(nodes: &mut [CategoryNode], worded: &mut HashMap<String, (String, String, String)>) {
        for node in nodes {
            if let Some((name, slug, description)) = worded.remove(&node.category.id) {
                node.category.name = name;
                node.category.slug = slug;
                said(description, &mut node.category.description);
            }
            walk(&mut node.children, worded);
        }
    }
    walk(nodes, &mut worded);
    Ok(())
}

/// What a category's page shows beside its listing, in `language`: the
/// translated guide and questions where the translation has them.
pub(crate) async fn hub_in(
    db: &Db,
    language: &str,
    category_id: &str,
    media_url: Option<&str>,
    hub: &mut CategoryHub,
) -> anyhow::Result<()> {
    if let Some(translation) =
        translation_of(db, TranslationKind::Category, category_id, language).await?
    {
        if !translation.body.trim().is_empty() {
            hub.guide_html = render_markdown(&translation.body, media_url.unwrap_or_default());
        }
        if !translation.questions.is_empty() {
            hub.questions = translation.questions;
        }
    }
    products_in(db, language, &mut hub.pinned).await
}

/// What search engines show for a thing's page in `language`, when it has
/// a translation: the translated words for them, else the translated name
/// and description (`name` and `description` are already the page's own in
/// that language). `None` leaves it to the main wording.
pub(crate) async fn seo_in(
    db: &Db,
    language: &str,
    kind: TranslationKind,
    id: &str,
    name: &str,
    description: &str,
) -> anyhow::Result<Option<Seo>> {
    Ok(translation_of(db, kind, id, language)
        .await?
        .map(|translation| {
            Seo::of(
                Some((translation.meta_title, translation.meta_description)),
                name,
                description,
            )
        }))
}

/// The slugs a product, a category or an article goes by: its own, in
/// the shop's main language, and the one of each language it is translated
/// into.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slugs {
    pub main: String,
    /// A language, then the slug there.
    pub translated: Vec<(String, String)>,
}

impl Slugs {
    /// The slug `language` reads it at: its translation's, or the main one
    /// where it has none.
    pub fn in_language(&self, language: &str) -> &str {
        self.translated
            .iter()
            .find(|(theirs, _)| theirs == language)
            .map_or(self.main.as_str(), |(_, slug)| slug.as_str())
    }
}

/// The slugs of the thing `id` of `kind` (`product`, `category` or
/// `article`); `None` when it is not shown.
pub(crate) async fn slugs_of(db: &Db, kind: &str, id: &str) -> anyhow::Result<Option<Slugs>> {
    let (rows, translations, key) = match kind {
        "product" => (
            "storefront_products",
            "storefront_translations",
            TRANSLATIONS_KEY,
        ),
        "category" => (
            "storefront_categories",
            "storefront_translations",
            TRANSLATIONS_KEY,
        ),
        _ => (
            "storefront_articles",
            "storefront_text_translations",
            TEXT_TRANSLATIONS_KEY,
        ),
    };
    let main: Option<(String,)> = db
        .fetch_optional(&format!("SELECT slug FROM {rows} WHERE id = ?1"), (id,))
        .await?;
    let Some((main,)) = main else {
        return Ok(None);
    };
    let translated = db
        .fetch_all(
            &format!(
                "SELECT language, slug FROM {translations} INDEXED BY {key}
                 WHERE kind = ?1 AND subject_id = ?2"
            ),
            (kind, id),
        )
        .await?;
    Ok(Some(Slugs { main, translated }))
}

/// Words a page in `language`, where it has a translation.
pub(crate) async fn page_in(
    db: &Db,
    language: &str,
    media_url: Option<&str>,
    page: &mut PublicPage,
) -> anyhow::Result<()> {
    let Some(translation) = text_translation_of(db, TextKind::Page, &page.id, language).await?
    else {
        return Ok(());
    };
    let TextTranslation {
        title,
        body,
        meta_title,
        meta_description,
        ..
    } = translation;
    if !body.trim().is_empty() {
        page.html = render_markdown(&body, media_url.unwrap_or_default());
    }
    page.meta_title = if meta_title.is_empty() {
        title.clone()
    } else {
        meta_title
    };
    if !meta_description.is_empty() {
        page.meta_description = meta_description;
    } else if !body.trim().is_empty() {
        page.meta_description = excerpt(&timada_pages::markdown_text(&body), DESCRIPTION_CHARS);
    }
    page.title = title;
    Ok(())
}

/// Words the blog's cards in `language`, where they have a translation.
pub(crate) async fn article_cards_in(
    db: &Db,
    language: &str,
    cards: &mut [ArticleCard],
) -> anyhow::Result<()> {
    for card in cards {
        if let Some(translation) =
            text_translation_of(db, TextKind::Article, &card.id, language).await?
        {
            card.title = translation.title;
            card.slug = translation.slug;
            said(translation.summary, &mut card.summary);
        }
    }
    Ok(())
}

/// Words an article in `language`, where it has a translation.
pub(crate) async fn article_in(
    db: &Db,
    language: &str,
    media_url: Option<&str>,
    article: &mut PublicArticle,
) -> anyhow::Result<()> {
    let Some(translation) =
        text_translation_of(db, TextKind::Article, &article.id, language).await?
    else {
        return Ok(());
    };
    if !translation.body.trim().is_empty() {
        let rendered = render_article(&translation.body, media_url.unwrap_or_default());
        article.blocks = rendered.blocks;
        article.headings = rendered.headings;
    }
    said(translation.summary, &mut article.summary);
    article.meta_title = if translation.meta_title.is_empty() {
        translation.title.clone()
    } else {
        translation.meta_title
    };
    if !translation.meta_description.is_empty() {
        article.meta_description = translation.meta_description;
    } else if !article.summary.is_empty() {
        article.meta_description = excerpt(&article.summary, DESCRIPTION_CHARS);
    }
    article.title = translation.title;
    article.slug = translation.slug;
    Ok(())
}
