//! A shop's published reviews as Google's product reviews feed reads them
//! (schema 2.4), so its Shopping listings show their stars.
//!
//! The feed is whole each time: a review that was in it and is no longer
//! (rejected after the fact) is dropped by Google at its next read.
//! [`published_reviews`] reads them a page at a time off the table's key;
//! the host says what each reviewed product is ([`ReviewedProduct`]: its
//! page, its numbers) and [`write_google_review`] writes one.

use std::fmt::Write;

use timada_core::db::Db;
use timada_core::xml::xml_text as xml;

use super::reviews::ReviewRow;

/// The schema the feed is written to.
pub const GOOGLE_REVIEWS_VERSION: &str = "2.4";

/// A page of the tenant's reviews after the review `after`, in id order,
/// read off the table's key: every one of them, whatever its state, so the
/// walk never sorts. The caller keeps the published ones
/// ([`ReviewRow::status`]) and goes on from the last id.
pub async fn reviews_page(
    db: &Db,
    tenant_id: &str,
    after: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<ReviewRow>> {
    db.fetch_all(REVIEWS_PAGE, (tenant_id, after.unwrap_or(""), limit))
        .await
}

const REVIEWS_PAGE: &str = "SELECT tenant_id, id, order_id, order_reference, product_id,
    product_name, variant_label, rating, title, body, author_name, photo_count, status,
    submitted_at, updated_at
    FROM reviews_reviews WHERE tenant_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3";

/// What a review's feed says of the product reviewed: where its page is
/// and what Google matches it to a listing by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewedProduct {
    /// The product's page in the shop, where its reviews show.
    pub url: String,
    pub name: String,
    pub gtins: Vec<String>,
    pub mpns: Vec<String>,
    /// The ids its items go by in the shop's shopping feed.
    pub skus: Vec<String>,
    pub brand: Option<String>,
}

/// The opening of the feed, up to the first review.
pub fn google_reviews_head(shop_name: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <feed xmlns:vc=\"http://www.w3.org/2007/XMLSchema-versioning\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:noNamespaceSchemaLocation=\"http://www.google.com/shopping/reviews/schema/product/{GOOGLE_REVIEWS_VERSION}/product_reviews.xsd\">\n\
         <version>{GOOGLE_REVIEWS_VERSION}</version>\n\
         <publisher>\n<name>{}</name>\n</publisher>\n<reviews>\n",
        xml(shop_name),
    )
}

/// What closes the feed after the last review.
pub const GOOGLE_REVIEWS_TAIL: &str = "</reviews>\n</feed>\n";

/// Unix seconds as the schema's `xs:dateTime` (`2026-11-15T10:00:00Z`).
fn date_time(secs: i64) -> String {
    let text = timada_core::civil::format_time(secs.max(0) as u64);
    format!("{}T{}Z", &text[..10], &text[11..19])
}

fn element(out: &mut String, name: &str, value: &str) {
    let _ = writeln!(out, "<{name}>{}</{name}>", xml(value));
}

/// `<gtins><gtin>…</gtin></gtins>`, when there is one to give.
fn list(out: &mut String, many: &str, one: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    let _ = writeln!(out, "<{many}>");
    for value in values {
        element(out, one, value);
    }
    let _ = writeln!(out, "</{many}>");
}

/// Writes one published review of `product`, in the schema's order. Every
/// review here was written by someone who bought the product from the
/// shop, asked after the order was fulfilled.
pub fn write_google_review(out: &mut String, review: &ReviewRow, product: &ReviewedProduct) {
    out.push_str("<review>\n");
    element(out, "review_id", &review.id);
    out.push_str("<reviewer>\n");
    let name = review.author_name.trim();
    if name.is_empty() {
        out.push_str("<name is_anonymous=\"true\">Anonymous</name>\n");
    } else {
        element(out, "name", name);
    }
    out.push_str("</reviewer>\n");
    element(out, "is_verified_purchase", "true");
    element(out, "review_timestamp", &date_time(review.submitted_at));
    if let Some(title) = review
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    {
        element(out, "title", title);
    }
    element(out, "content", &review.body);
    let _ = writeln!(
        out,
        "<review_url type=\"group\">{}</review_url>",
        xml(&product.url)
    );
    let _ = writeln!(
        out,
        "<ratings>\n<overall min=\"1\" max=\"5\">{}</overall>\n</ratings>",
        review.rating.clamp(1, 5)
    );
    out.push_str("<products>\n<product>\n");
    let brands: Vec<String> = product.brand.iter().cloned().collect();
    if !(product.gtins.is_empty()
        && product.mpns.is_empty()
        && product.skus.is_empty()
        && brands.is_empty())
    {
        out.push_str("<product_ids>\n");
        list(out, "gtins", "gtin", &product.gtins);
        list(out, "mpns", "mpn", &product.mpns);
        list(out, "skus", "sku", &product.skus);
        list(out, "brands", "brand", &brands);
        out.push_str("</product_ids>\n");
    }
    element(out, "product_name", &product.name);
    element(out, "product_url", &product.url);
    out.push_str("</product>\n</products>\n");
    element(out, "collection_method", "post_fulfillment");
    element(out, "transaction_id", &review.order_reference);
    out.push_str("</review>\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review() -> ReviewRow {
        ReviewRow {
            tenant_id: "acme".into(),
            id: "01REVIEW".into(),
            order_id: "order-1".into(),
            order_reference: "K7Q4-MZP2".into(),
            product_id: "01MUG".into(),
            product_name: "Blue Mug".into(),
            variant_label: None,
            rating: 4,
            title: Some("Solid & pretty".into()),
            body: "Holds <a lot>.".into(),
            author_name: "Ada L.".into(),
            photo_count: 0,
            status: "published".into(),
            submitted_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        }
    }

    fn product() -> ReviewedProduct {
        ReviewedProduct {
            url: "https://shop.example/p/blue-mug".into(),
            name: "Blue Mug".into(),
            gtins: vec!["4006381333931".into()],
            mpns: Vec::new(),
            skus: vec!["01MUG".into()],
            brand: Some("Acme".into()),
        }
    }

    #[test]
    fn a_review_is_written_in_the_schemas_order() {
        let mut out = String::new();
        write_google_review(&mut out, &review(), &product());
        assert_eq!(
            out,
            "<review>\n\
             <review_id>01REVIEW</review_id>\n\
             <reviewer>\n<name>Ada L.</name>\n</reviewer>\n\
             <is_verified_purchase>true</is_verified_purchase>\n\
             <review_timestamp>2023-11-14T22:13:20Z</review_timestamp>\n\
             <title>Solid &amp; pretty</title>\n\
             <content>Holds &lt;a lot&gt;.</content>\n\
             <review_url type=\"group\">https://shop.example/p/blue-mug</review_url>\n\
             <ratings>\n<overall min=\"1\" max=\"5\">4</overall>\n</ratings>\n\
             <products>\n<product>\n\
             <product_ids>\n\
             <gtins>\n<gtin>4006381333931</gtin>\n</gtins>\n\
             <skus>\n<sku>01MUG</sku>\n</skus>\n\
             <brands>\n<brand>Acme</brand>\n</brands>\n\
             </product_ids>\n\
             <product_name>Blue Mug</product_name>\n\
             <product_url>https://shop.example/p/blue-mug</product_url>\n\
             </product>\n</products>\n\
             <collection_method>post_fulfillment</collection_method>\n\
             <transaction_id>K7Q4-MZP2</transaction_id>\n\
             </review>\n"
        );
    }

    #[test]
    fn what_a_review_lacks_is_left_out() {
        let mut out = String::new();
        write_google_review(
            &mut out,
            &ReviewRow {
                title: Some("  ".into()),
                author_name: " ".into(),
                ..review()
            },
            &ReviewedProduct {
                gtins: Vec::new(),
                skus: Vec::new(),
                brand: None,
                ..product()
            },
        );
        assert!(out.contains("<name is_anonymous=\"true\">Anonymous</name>"));
        assert!(!out.contains("<title>"));
        assert!(!out.contains("<product_ids>"));
    }

    #[test]
    fn the_head_names_the_schema_and_the_shop() {
        let head = google_reviews_head("Acme & Co");
        assert!(head.contains("/product/2.4/product_reviews.xsd"));
        assert!(head.contains("<version>2.4</version>"));
        assert!(head.contains("<publisher>\n<name>Acme &amp; Co</name>\n</publisher>"));
        assert!(head.ends_with("<reviews>\n"));
    }

    /// The feed walks a shop's every review: each page is a walk of the
    /// table's key, never a scan of every shop's rows nor a sort.
    #[tokio::test]
    async fn a_page_is_read_off_the_key() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        crate::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        let db = Db::from(store);
        let plan: Vec<(i64, i64, i64, String)> = db
            .fetch_all(
                &format!("EXPLAIN QUERY PLAN {REVIEWS_PAGE}"),
                ("acme", "", 200_i64),
            )
            .await
            .unwrap();
        let plan: Vec<String> = plan.into_iter().map(|row| row.3).collect();
        assert!(plan.iter().any(|step| step.contains("SEARCH")), "{plan:?}");
        assert!(
            !plan
                .iter()
                .any(|step| step.contains("SCAN") || step.contains("TEMP B-TREE")),
            "{plan:?}"
        );
        assert!(
            reviews_page(&db, "acme", None, 200)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
