//! A shop's feeds, written a page at a time: its products as Google
//! Merchant Center, a Meta catalogue or a TikTok catalogue reads them, and
//! its published reviews as Google's product ratings read them.
//!
//! The rows come from the read models ([`timada_catalog::feed_products`],
//! [`timada_reviews::reviews_page`]) and their crates write the items; this
//! module adds what only the dashboard knows (where the shop is, what
//! delivery costs, where the pictures are served from, the VAT a margin is
//! graded with, which product a review is of) and sends the XML as it is written, so a shop of any size
//! is answered without being held in memory.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::Frame;
use std::collections::HashMap;

use timada_catalog::{
    FeedReader, FeedShipping, FeedShop, GOOGLE_FEED_TAIL, default_delivery_days, feed_product,
    feed_products, google_feed_head, product_wording, write_feed_items,
};
use timada_core::Rendition;
use timada_reviews::{
    GOOGLE_REVIEWS_TAIL, ReviewStatus, ReviewedProduct, google_reviews_head, reviews_page,
    write_google_review,
};
use timada_tenancy::TenantState;
use tokio::sync::mpsc;
use topcoat::router::Body;

use crate::cluster::Executor;
use crate::db::Db;
use crate::media::Media;

/// How many products one read of the feed's rows takes.
const PAGE: i64 = 200;

/// What ends a feed half-way: a reader must not take the part it got for
/// the whole.
pub(crate) type Broken = Box<dyn std::error::Error + Send + Sync>;

/// What a feed says of the shop beside its products, or `None` when the
/// merchant has not said where the shop is: a feed with no link to a
/// product's page is no feed.
pub async fn shop_of(
    store: &Executor,
    db: &Db,
    tenant: &TenantState,
) -> anyhow::Result<Option<FeedShop>> {
    let orders = timada_orders::Command::new(store.clone(), &tenant.id);
    let Some(storefront_url) = orders.storefront_address().await? else {
        return Ok(None);
    };
    let shippings = orders
        .shipping_methods()
        .await?
        .iter()
        .flat_map(|method| {
            method.countries.iter().map(|country| FeedShipping {
                country: country.clone(),
                service: method.name.clone(),
                price_cents: method.price_cents,
                currency: method.currency.clone(),
                free_above_cents: method.free_above_cents,
            })
        })
        .collect();
    let costs = timada_catalog::load_sale_costs(store, &tenant.id).await?;
    let ads = match timada_catalog::Command::new(store.clone(), &tenant.id)
        .load_feed_ads_label()
        .await?
    {
        Some(label) => Some(timada_catalog::FeedAds {
            label: usize::from(label),
            categories: timada_catalog::categories_out_of_ads(db, &tenant.id).await?,
        }),
        None => None,
    };
    Ok(Some(FeedShop {
        name: tenant.name.clone(),
        storefront_url,
        paths: timada_catalog::category_paths(db, &tenant.id).await?,
        prefix: String::new(),
        shippings,
        delivery_days: default_delivery_days(db, &tenant.id).await?,
        margin: crate::margins::feed_margin(store, tenant, &costs).await?,
        ads,
    }))
}

/// A country and a language the shop sells in: what one feed is for. The
/// countries are the ones its shipping methods name, the languages the
/// ones its storefront is read in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Market {
    /// Two letters, upper case: `BE`.
    pub country: String,
    /// An ISO 639-1 code: `fr`.
    pub language: String,
}

impl Market {
    /// What a feed's address names it by: `be-fr`.
    pub fn code(&self) -> String {
        format!("{}-{}", self.country.to_ascii_lowercase(), self.language)
    }

    /// The market an address names, whatever the shop sells in.
    pub fn parse(code: &str) -> Option<Self> {
        let (country, language) = code.split_once('-')?;
        let valid = |part: &str| part.len() == 2 && part.bytes().all(|b| b.is_ascii_lowercase());
        (valid(country) && valid(language)).then(|| Market {
            country: country.to_ascii_uppercase(),
            language: language.to_owned(),
        })
    }
}

/// The markets the shop has a feed for: each country its shipping methods
/// name, in each language its storefront is read in (the main one first).
/// None for a shop that never chose its languages, or whose methods name no
/// country: its one feed, in the main language, is all it has.
pub async fn markets_of(store: &Executor, tenant: &TenantState) -> anyhow::Result<Vec<Market>> {
    let Some(languages) = timada_pages::Command::new(store.clone(), &tenant.id)
        .languages()
        .await?
    else {
        return Ok(Vec::new());
    };
    let mut countries: Vec<String> = timada_orders::Command::new(store.clone(), &tenant.id)
        .shipping_methods()
        .await?
        .into_iter()
        .flat_map(|method| method.countries)
        .collect();
    countries.sort();
    countries.dedup();
    Ok(countries
        .iter()
        .flat_map(|country| {
            std::iter::once(&languages.main)
                .chain(&languages.others)
                .map(|language| Market {
                    country: country.clone(),
                    language: language.clone(),
                })
        })
        .collect())
}

/// What a feed in another language words its products with.
pub struct FeedWording {
    language: String,
    /// What the language reads for the words the products share.
    words: HashMap<String, String>,
}

/// `shop` as the feed of `market` says it, or `None` when the shop does
/// not sell there: the delivery prices of that country alone, each method
/// named as the language names it, and, in a language other than the main
/// one, each product at its address there, worded as it is read there.
pub async fn in_market(
    store: &Executor,
    db: &Db,
    tenant: &TenantState,
    mut shop: FeedShop,
    market: &Market,
) -> anyhow::Result<Option<(FeedShop, Option<FeedWording>)>> {
    if !markets_of(store, tenant).await?.contains(market) {
        return Ok(None);
    }
    shop.shippings
        .retain(|shipping| shipping.country == market.country);
    let main = timada_pages::Command::new(store.clone(), &tenant.id)
        .languages()
        .await?
        .is_none_or(|languages| languages.main == market.language);
    if main {
        return Ok(Some((shop, None)));
    }
    // A method's name in the language, by the name it has in the main one:
    // a shop's methods are few, and a feed's lines carry no id.
    let orders = timada_orders::Command::new(store.clone(), &tenant.id);
    let named = orders.shipping_names(&market.language).await?;
    for method in orders.shipping_methods().await? {
        if let Some(theirs) = named.get(&method.id) {
            for shipping in &mut shop.shippings {
                if shipping.service == method.name {
                    shipping.service = theirs.clone();
                }
            }
        }
    }
    shop.paths = timada_catalog::category_paths_in(db, &tenant.id, &market.language).await?;
    shop.prefix = format!("/{}", market.language);
    let words = timada_catalog::words_in(db, &tenant.id, &market.language).await?;
    Ok(Some((
        shop,
        Some(FeedWording {
            language: market.language.clone(),
            words,
        }),
    )))
}

/// The products of `tenant_id` as `reader` takes them, as a response body
/// written behind the response: each page of products is read when the
/// reader has taken the one before. A read that fails breaks the body off
/// rather than end it early.
///
/// `wording` is what a feed in another language words them with: a
/// product with no translation there is listed as the main language says
/// it, as the storefront reads it.
pub fn products_feed(
    reader: FeedReader,
    db: Db,
    media: Arc<Media>,
    tenant_id: String,
    shop: FeedShop,
    wording: Option<FeedWording>,
) -> Body {
    Body::new(pages_of(reader, db, media, tenant_id, shop, wording, PAGE))
}

/// What a reviews feed says of the product `id`, when the shop's shopping
/// feed carries it: its page, and the numbers and ids its items go by
/// there, so Google matches the review to the listing.
async fn reviewed_product(
    db: &Db,
    tenant_id: &str,
    shop: &FeedShop,
    id: &str,
) -> anyhow::Result<Option<ReviewedProduct>> {
    let Some(product) = feed_product(db, tenant_id, id).await? else {
        return Ok(None);
    };
    let each = |own: &Option<String>, of: fn(&timada_catalog::FeedVariant) -> &Option<String>| {
        let mut values: Vec<String> = own
            .iter()
            .chain(product.variants.iter().filter_map(|v| of(v).as_ref()))
            .cloned()
            .collect();
        values.sort();
        values.dedup();
        values
    };
    let skus = if product.variants.is_empty() {
        vec![product.id.clone()]
    } else {
        product.variants.iter().map(|v| v.id.clone()).collect()
    };
    Ok(Some(ReviewedProduct {
        url: format!(
            "{}{}",
            shop.storefront_url,
            shop.paths.product(&product.categories, &product.slug)
        ),
        name: product.name.clone(),
        gtins: each(&product.gtin, |v| &v.gtin),
        mpns: each(&product.mpn, |v| &v.mpn),
        skus,
        brand: product.brand.clone(),
    }))
}

/// The published reviews of `tenant_id` as Google's product reviews feed,
/// written behind the response a page at a time, as [`products_feed`] is.
/// A review of a product the shopping feed does not carry (unpublished, or
/// taken out of it) is left out: it has no listing to show on.
pub fn reviews_feed(db: Db, tenant_id: String, shop: FeedShop) -> Body {
    Body::new(review_pages_of(db, tenant_id, shop, PAGE))
}

/// [`reviews_feed`], reading `page` reviews at a time.
fn review_pages_of(db: Db, tenant_id: String, shop: FeedShop, page: i64) -> Pages {
    let (pages, body) = mpsc::channel::<Result<Bytes, Broken>>(1);
    tokio::spawn(async move {
        if pages
            .send(Ok(google_reviews_head(&shop.name).into()))
            .await
            .is_err()
        {
            return;
        }
        // The products the reviews read so far are of; `None` for one the
        // shopping feed does not carry.
        let mut products: HashMap<String, Option<ReviewedProduct>> = HashMap::new();
        let mut after: Option<String> = None;
        loop {
            let written = async {
                let reviews = reviews_page(&db, &tenant_id, after.as_deref(), page).await?;
                let mut xml = String::new();
                for review in &reviews {
                    if review.status != ReviewStatus::Published.as_str() {
                        continue;
                    }
                    if !products.contains_key(&review.product_id) {
                        let product =
                            reviewed_product(&db, &tenant_id, &shop, &review.product_id).await?;
                        products.insert(review.product_id.clone(), product);
                    }
                    if let Some(Some(product)) = products.get(&review.product_id) {
                        write_google_review(&mut xml, review, product);
                    }
                }
                anyhow::Ok((reviews.last().map(|review| review.id.clone()), xml))
            };
            let (last, xml) = match written.await {
                Ok(written) => written,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, %err, "a reviews feed broke off");
                    let _ = pages.send(Err(err.into())).await;
                    return;
                }
            };
            let Some(last) = last else {
                break;
            };
            after = Some(last);
            // The reader went away: nothing more to write.
            if !xml.is_empty() && pages.send(Ok(xml.into())).await.is_err() {
                return;
            }
        }
        let _ = pages
            .send(Ok(Bytes::from_static(GOOGLE_REVIEWS_TAIL.as_bytes())))
            .await;
    });
    Pages(body)
}

/// [`products_feed`], reading `page` products at a time.
fn pages_of(
    reader: FeedReader,
    db: Db,
    media: Arc<Media>,
    tenant_id: String,
    shop: FeedShop,
    wording: Option<FeedWording>,
    page: i64,
) -> Pages {
    let (pages, body) = mpsc::channel::<Result<Bytes, Broken>>(1);
    tokio::spawn(async move {
        if pages
            .send(Ok(google_feed_head(&shop).into()))
            .await
            .is_err()
        {
            return;
        }
        let mut after: Option<String> = None;
        loop {
            let read = async {
                let mut products = feed_products(&db, &tenant_id, after.as_deref(), page).await?;
                // The id a page is read after is the product's own, whatever
                // it is called in the language.
                let last = products.last().map(|product| product.id.clone());
                if let Some(wording) = &wording {
                    let mut worded = Vec::with_capacity(products.len());
                    for product in products {
                        let theirs =
                            product_wording(&db, &tenant_id, &product.id, &wording.language)
                                .await?;
                        worded.push(product.worded(theirs, &wording.words));
                    }
                    products = worded;
                }
                anyhow::Ok((products, last))
            };
            let (products, last) = match read.await {
                Ok(read) => read,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, %err, "a shopping feed broke off");
                    let _ = pages.send(Err(err.into())).await;
                    return;
                }
            };
            let Some(last) = last else {
                break;
            };
            after = Some(last);
            let mut xml = String::new();
            for product in &products {
                write_feed_items(&mut xml, reader, product, &shop, |image_id| {
                    media.url(&tenant_id, image_id, Rendition::Page)
                });
            }
            // The reader went away: nothing more to write.
            if pages.send(Ok(xml.into())).await.is_err() {
                return;
            }
        }
        let _ = pages
            .send(Ok(Bytes::from_static(GOOGLE_FEED_TAIL.as_bytes())))
            .await;
    });
    Pages(body)
}

/// The pages of a feed as they are written.
pub(crate) struct Pages(mpsc::Receiver<Result<Bytes, Broken>>);

impl Pages {
    /// The body that gives what is sent on the other end of `pages`.
    pub(crate) fn of(pages: mpsc::Receiver<Result<Bytes, Broken>>) -> Self {
        Self(pages)
    }
}

impl http_body::Body for Pages {
    type Data = Bytes;
    type Error = Broken;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Broken>>> {
        self.0
            .poll_recv(cx)
            .map(|page| page.map(|page| page.map(Frame::data)))
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use http_body::Body as _;
    use timada_catalog::{Command, CreateProduct};

    use super::*;

    const TENANT: &str = "tenant-1";

    fn media() -> Arc<Media> {
        Arc::new(Media::new(
            &crate::config::MediaConfig {
                dir: std::env::temp_dir(),
                public_url: "https://media.example".to_owned(),
                tenant_quota_bytes: 0,
                reconcile_check_secs: 86_400,
            },
            None,
        ))
    }

    fn shop() -> FeedShop {
        FeedShop {
            name: "Acme".into(),
            storefront_url: "https://shop.example".into(),
            paths: Default::default(),
            prefix: String::new(),
            ..Default::default()
        }
    }

    /// Everything the body gives, and whether it ended whole.
    async fn read(mut pages: Pages) -> (String, bool) {
        let mut xml = String::new();
        loop {
            let frame = poll_fn(|cx| Pin::new(&mut pages).poll_frame(cx)).await;
            match frame {
                Some(Ok(frame)) => {
                    xml.push_str(std::str::from_utf8(frame.data_ref().unwrap()).unwrap())
                }
                Some(Err(_)) => return (xml, false),
                None => return (xml, true),
            }
        }
    }

    #[tokio::test]
    async fn a_feed_is_written_whole_a_page_after_the_other() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        timada_catalog::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        let db = Db::from(store.clone());
        let catalog = Command::new(store.clone(), TENANT);
        let mut slugs = Vec::new();
        for n in 0..5 {
            let product = catalog
                .create_product(
                    "user-alice",
                    CreateProduct {
                        name: format!("Mug {n}"),
                        slug: None,
                    },
                )
                .await
                .unwrap();
            catalog
                .price_product("user-alice", &product.id, 1250, "EUR")
                .await
                .unwrap();
            catalog
                .publish_product("user-alice", &product.id)
                .await
                .unwrap();
            slugs.push(product.slug);
        }
        timada_catalog::read_model_subscription::<evento::Turso>(db.clone())
            .run_once(&store)
            .await
            .unwrap();

        // Two products a read: three pages for five.
        let (xml, whole) = read(pages_of(
            FeedReader::Google,
            db,
            media(),
            TENANT.into(),
            shop(),
            None,
            2,
        ))
        .await;
        assert!(whole);
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.ends_with("</channel>\n</rss>\n"));
        assert_eq!(xml.matches("<item>").count(), 5);
        assert_eq!(xml.matches("</item>").count(), 5);
        for slug in &slugs {
            let link = format!("<g:link>https://shop.example/p/{slug}</g:link>");
            assert_eq!(xml.matches(&link).count(), 1, "{slug}");
        }
    }

    /// A feed in another language: a product at its address there, under
    /// its category's slug there, worded as its translation says, its
    /// combinations by the shop's words; one with no translation as the
    /// main language says it.
    #[tokio::test]
    async fn a_feed_in_another_language_words_and_links_as_it_is_read_there() {
        use timada_catalog::{
            CreateCategory, DefineVariant, OptionSelection, TranslationKind, TranslationText,
        };

        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        timada_catalog::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        let db = Db::from(store.clone());
        let catalog = Command::new(store.clone(), TENANT);
        let cats = catalog
            .create_category(
                "user-alice",
                CreateCategory {
                    name: "Chats".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let named = |name: &str| TranslationText {
            name: name.into(),
            ..Default::default()
        };
        catalog
            .translate(
                "user-alice",
                TranslationKind::Category,
                &cats.id,
                "en",
                named("Cats"),
            )
            .await
            .unwrap();
        let create = async |name: &str| {
            let product = catalog
                .create_product(
                    "user-alice",
                    CreateProduct {
                        name: name.into(),
                        slug: None,
                    },
                )
                .await
                .unwrap();
            catalog
                .categorize_product("user-alice", &product.id, &cats.id)
                .await
                .unwrap();
            product.id
        };
        // A collar in one colour, worded in English.
        let collar = create("Collier").await;
        let colour = catalog
            .define_option("user-alice", &collar, "Couleur", vec!["Rose".into()])
            .await
            .unwrap();
        catalog
            .define_variant(
                "user-alice",
                &collar,
                DefineVariant {
                    selections: vec![OptionSelection {
                        option_id: colour,
                        value: "Rose".into(),
                    }],
                    sku: None,
                    amount_cents: 990,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap();
        catalog
            .translate(
                "user-alice",
                TranslationKind::Product,
                &collar,
                "en",
                TranslationText {
                    description: "A collar for a cat.".into(),
                    ..named("Collar")
                },
            )
            .await
            .unwrap();
        catalog
            .word("user-alice", "en", "Rose", "Pink")
            .await
            .unwrap();
        // A bowl with no English wording.
        let bowl = create("Gamelle").await;
        catalog
            .price_product("user-alice", &bowl, 500, "EUR")
            .await
            .unwrap();
        for id in [&collar, &bowl] {
            catalog.publish_product("user-alice", id).await.unwrap();
        }
        timada_catalog::read_model_subscription::<evento::Turso>(db.clone())
            .run_once(&store)
            .await
            .unwrap();

        let english = FeedShop {
            paths: timada_catalog::category_paths_in(&db, TENANT, "en")
                .await
                .unwrap(),
            prefix: "/en".into(),
            ..shop()
        };
        let wording = FeedWording {
            language: "en".into(),
            words: timada_catalog::words_in(&db, TENANT, "en").await.unwrap(),
        };
        let (xml, whole) = read(pages_of(
            FeedReader::Google,
            db.clone(),
            media(),
            TENANT.into(),
            english,
            Some(wording),
            1,
        ))
        .await;
        assert!(whole);
        assert!(
            xml.contains("<g:link>https://shop.example/en/cats/collar?v="),
            "{xml}"
        );
        assert!(xml.contains("<g:title>Collar – Pink</g:title>"), "{xml}");
        assert!(xml.contains("A collar for a cat."), "{xml}");
        assert!(
            xml.contains("<g:link>https://shop.example/en/cats/gamelle</g:link>"),
            "{xml}"
        );
        assert!(xml.contains("<g:title>Gamelle</g:title>"), "{xml}");
        assert!(!xml.contains("Collier") && !xml.contains("Rose"), "{xml}");

        // The main language's feed is as it was.
        let french = FeedShop {
            paths: timada_catalog::category_paths(&db, TENANT).await.unwrap(),
            ..shop()
        };
        let (xml, _) = read(pages_of(
            FeedReader::Google,
            db,
            media(),
            TENANT.into(),
            french,
            None,
            2,
        ))
        .await;
        assert!(
            xml.contains("<g:link>https://shop.example/chats/collier?v="),
            "{xml}"
        );
        assert!(xml.contains("<g:title>Collier – Rose</g:title>"), "{xml}");
    }

    #[test]
    fn a_market_is_a_country_then_a_language() {
        let market = Market::parse("be-fr").unwrap();
        assert_eq!(
            (market.country.as_str(), market.language.as_str()),
            ("BE", "fr")
        );
        assert_eq!(market.code(), "be-fr");
        for not_one in ["google", "BE-fr", "be", "be-fra", "be_fr", ""] {
            assert!(Market::parse(not_one).is_none(), "{not_one}");
        }
    }

    #[tokio::test]
    async fn a_read_that_fails_breaks_the_feed_off() {
        // No table to read: the first page fails.
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let (xml, whole) = read(pages_of(
            FeedReader::Google,
            Db::from(store),
            media(),
            TENANT.into(),
            shop(),
            None,
            2,
        ))
        .await;
        assert!(!whole, "a reader must not take a part for the whole");
        assert!(!xml.contains("</rss>"));
    }

    #[tokio::test]
    async fn the_reviews_feed_carries_the_published_reviews_of_products_in_the_feed() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        for migrations in [&timada_catalog::MIGRATIONS, &timada_reviews::MIGRATIONS] {
            migrations
                .to_latest(&mut conn)
                .await
                .map_err(|(_, err)| err)
                .unwrap();
        }
        let db = Db::from(store.clone());
        let catalog = Command::new(store.clone(), TENANT);
        let mug = catalog
            .create_product(
                "user-alice",
                CreateProduct {
                    name: "Blue Mug".into(),
                    slug: None,
                },
            )
            .await
            .unwrap();
        catalog
            .price_product("user-alice", &mug.id, 1250, "EUR")
            .await
            .unwrap();
        catalog
            .publish_product("user-alice", &mug.id)
            .await
            .unwrap();
        timada_catalog::read_model_subscription::<evento::Turso>(db.clone())
            .run_once(&store)
            .await
            .unwrap();
        // Three published reviews of the mug, one still pending, one of
        // another shop's, and one of a product no feed carries.
        for (tenant, id, product, status) in [
            (TENANT, "r1", mug.id.as_str(), "published"),
            (TENANT, "r2", mug.id.as_str(), "pending"),
            (TENANT, "r3", mug.id.as_str(), "published"),
            (TENANT, "r4", "gone", "published"),
            (TENANT, "r5", mug.id.as_str(), "published"),
            ("tenant-2", "r6", mug.id.as_str(), "published"),
        ] {
            db.execute(
                "INSERT INTO reviews_reviews (tenant_id, id, order_id, order_reference,
                    product_id, product_name, variant_label, rating, title, body, author_name,
                    photo_count, status, submitted_at, updated_at)
                 VALUES (?1, ?2, 'order-1', 'K7Q4-MZP2', ?3, 'Blue Mug', NULL, 5, NULL,
                    'Holds a lot.', 'Ada L.', 0, ?4, 1700000000, 1700000000)",
                (tenant, id, product, status),
            )
            .await
            .unwrap();
        }

        // Two reviews a read: three pages for the shop's five.
        let (xml, whole) = read(review_pages_of(db, TENANT.into(), shop(), 2)).await;
        assert!(whole);
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.ends_with("</reviews>\n</feed>\n"));
        assert_eq!(xml.matches("<review>").count(), 3, "{xml}");
        for id in ["r1", "r3", "r5"] {
            assert!(
                xml.contains(&format!("<review_id>{id}</review_id>")),
                "{id}"
            );
        }
        let page = format!(
            "<product_url>https://shop.example/p/{}</product_url>",
            mug.slug
        );
        assert_eq!(xml.matches(&page).count(), 3);
        // The item's id in the shopping feed, for Google to match by.
        assert!(xml.contains(&format!("<sku>{}</sku>", mug.id)));
    }

    #[tokio::test]
    async fn the_other_catalogues_read_the_same_products() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        timada_catalog::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        let db = Db::from(store.clone());
        let catalog = Command::new(store.clone(), TENANT);
        let mug = catalog
            .create_product(
                "user-alice",
                CreateProduct {
                    name: "Blue Mug".into(),
                    slug: None,
                },
            )
            .await
            .unwrap();
        catalog
            .price_product("user-alice", &mug.id, 1250, "EUR")
            .await
            .unwrap();
        catalog
            .publish_product("user-alice", &mug.id)
            .await
            .unwrap();
        timada_catalog::read_model_subscription::<evento::Turso>(db.clone())
            .run_once(&store)
            .await
            .unwrap();
        for (reader, id) in [(FeedReader::Meta, "g:id"), (FeedReader::Tiktok, "g:sku_id")] {
            let (xml, whole) = read(pages_of(
                reader,
                db.clone(),
                media(),
                TENANT.into(),
                shop(),
                None,
                2,
            ))
            .await;
            assert!(whole);
            assert!(xml.contains(&format!("<{id}>{}</{id}>", mug.id)), "{xml}");
            assert!(xml.contains("<g:availability>in stock</g:availability>"));
            // No brand on the product: the shop's name.
            assert!(xml.contains("<g:brand>Acme</g:brand>"));
            assert!(xml.ends_with("</channel>\n</rss>\n"));
        }
    }
}
