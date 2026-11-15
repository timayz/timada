//! A shop's shopping feed as Google Merchant Center reads it, written a
//! page of products at a time.
//!
//! The rows come from the catalog's read model ([`timada_catalog::feed_products`])
//! and the catalog writes the items; this module adds what only the
//! dashboard knows (where the shop is, what delivery costs, where the
//! pictures are served from) and sends the XML as it is written, so a shop
//! of any size is answered without being held in memory.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::Frame;
use timada_catalog::{
    FeedShipping, FeedShop, GOOGLE_FEED_TAIL, default_delivery_days, feed_products,
    google_feed_head, write_google_items,
};
use timada_core::Rendition;
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
type Broken = Box<dyn std::error::Error + Send + Sync>;

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
    Ok(Some(FeedShop {
        name: tenant.name.clone(),
        storefront_url,
        shippings,
        delivery_days: default_delivery_days(db, &tenant.id).await?,
    }))
}

/// The feed of `tenant_id` as a response body, written behind the response:
/// each page of products is read when the reader has taken the one before.
/// A read that fails breaks the body off rather than end it early.
pub fn google_feed(db: Db, media: Arc<Media>, tenant_id: String, shop: FeedShop) -> Body {
    Body::new(pages_of(db, media, tenant_id, shop, PAGE))
}

/// [`google_feed`], reading `page` products at a time.
fn pages_of(db: Db, media: Arc<Media>, tenant_id: String, shop: FeedShop, page: i64) -> Pages {
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
            let products = match feed_products(&db, &tenant_id, after.as_deref(), page).await {
                Ok(products) => products,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, %err, "a shopping feed broke off");
                    let _ = pages.send(Err(err.into())).await;
                    return;
                }
            };
            let Some(last) = products.last() else {
                break;
            };
            after = Some(last.id.clone());
            let mut xml = String::new();
            for product in &products {
                write_google_items(&mut xml, product, &shop, |image_id| {
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
struct Pages(mpsc::Receiver<Result<Bytes, Broken>>);

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
        let (xml, whole) = read(pages_of(db, media(), TENANT.into(), shop(), 2)).await;
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

    #[tokio::test]
    async fn a_read_that_fails_breaks_the_feed_off() {
        // No table to read: the first page fails.
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let (xml, whole) = read(pages_of(Db::from(store), media(), TENANT.into(), shop(), 2)).await;
        assert!(!whole, "a reader must not take a part for the whole");
        assert!(!xml.contains("</rss>"));
    }
}
