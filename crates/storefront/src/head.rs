//! What a page's `<head>` tells a search engine or a network that shows a
//! link to it: the tags a shared link is drawn from ([`SocialCard`]) and
//! the structured data a search result is enriched with ([`product_ld`],
//! [`breadcrumb_ld`], [`organization_ld`], [`website_ld`]). Structured data
//! says what the page shows and nothing more: a rating only where the
//! reviews are, a price only where one is read.
//!
//! The host writes its own `<head>`, so these give the content: tags to
//! loop over, and JSON to put in a `<script type="application/ld+json">`
//! through [`json_ld`].

use serde_json::{Map, Value, json};
use timada_core::civil::format_date;

use crate::query::{Product, Variant};
use crate::reviews::Review;
use crate::terms::OfferTerms;

/// One `<meta>` of a shared link's card. Open Graph's are named by a
/// `property` attribute, Twitter's by `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaTag {
    /// Whether the key goes in `property` (Open Graph) rather than `name`.
    pub property: bool,
    pub key: &'static str,
    pub content: String,
}

/// What a page gives a network that draws a card for a link to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SocialCard<'a> {
    pub title: &'a str,
    /// Left out when empty.
    pub description: &'a str,
    /// The page's canonical address, whole.
    pub url: &'a str,
    /// The shop's name.
    pub site_name: &'a str,
    /// A picture's whole address, when the page has one to show.
    pub image: Option<&'a str>,
    /// What the page sells, in minor units of a currency: makes the card a
    /// product's.
    pub price: Option<(i64, &'a str)>,
}

impl SocialCard<'_> {
    /// The Open Graph and Twitter tags, in the order a head lists them.
    pub fn tags(&self) -> Vec<MetaTag> {
        let og = |key, content: &str| MetaTag {
            property: true,
            key,
            content: content.to_owned(),
        };
        let twitter = |key, content: &str| MetaTag {
            property: false,
            key,
            content: content.to_owned(),
        };
        let mut tags = vec![
            og(
                "og:type",
                if self.price.is_some() {
                    "product"
                } else {
                    "website"
                },
            ),
            og("og:title", self.title),
            og("og:url", self.url),
            og("og:site_name", self.site_name),
        ];
        if !self.description.is_empty() {
            tags.push(og("og:description", self.description));
        }
        if let Some(image) = self.image {
            tags.push(og("og:image", image));
        }
        if let Some((cents, currency)) = self.price {
            tags.push(og("product:price:amount", &decimal(cents)));
            tags.push(og("product:price:currency", currency));
        }
        tags.push(twitter(
            "twitter:card",
            if self.image.is_some() {
                "summary_large_image"
            } else {
                "summary"
            },
        ));
        tags.push(twitter("twitter:title", self.title));
        if !self.description.is_empty() {
            tags.push(twitter("twitter:description", self.description));
        }
        if let Some(image) = self.image {
            tags.push(twitter("twitter:image", image));
        }
        tags
    }
}

/// An amount in minor units as structured data writes it: `1990` is
/// `19.90`.
fn decimal(cents: i64) -> String {
    let cents = cents.max(0);
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// `value` as the text of a `<script type="application/ld+json">`: JSON in
/// which nothing can close the script or open a comment.
pub fn json_ld(value: &Value) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// What [`product_ld`] needs beside the product: where its page is and
/// what of it the page shows.
pub struct ProductPageLd<'a> {
    /// The product's canonical address, whole.
    pub url: &'a str,
    /// The description the page gives search engines; left out when empty.
    pub description: &'a str,
    /// The published reviews this page shows. Only the shop's own, shown
    /// here: a search engine checks the markup against the page.
    pub reviews: &'a [Review],
    /// What the shop says of delivery and returns
    /// ([`offer_terms`](crate::offer_terms)), written on every offer.
    pub terms: &'a OfferTerms,
}

/// A product's page as schema.org data: a `Product` with its offer, or a
/// `ProductGroup` with a `Product` per variant on sale, each at the address
/// `variant_url` gives it (the page showing that variant, so a search
/// engine can list it on its own). Pictures are the page renditions. The
/// rating and the reviews are there only when the product has some.
pub fn product_ld(
    product: &Product,
    page: &ProductPageLd<'_>,
    variant_url: impl Fn(&Variant) -> String,
) -> Value {
    let mut ld = Map::new();
    ld.insert("@context".into(), json!("https://schema.org"));
    ld.insert("name".into(), json!(product.name));
    ld.insert("url".into(), json!(page.url));
    if !page.description.is_empty() {
        ld.insert("description".into(), json!(page.description));
    }
    let images: Vec<&str> = product.images.iter().map(|i| i.url.as_str()).collect();
    if !images.is_empty() {
        ld.insert("image".into(), json!(images));
    }
    if let Some(brand) = &product.brand {
        ld.insert("brand".into(), json!({ "@type": "Brand", "name": brand }));
    }
    if product.variants.is_empty() {
        ld.insert("@type".into(), json!("Product"));
        ld.insert("sku".into(), json!(product.id));
        identifiers(&mut ld, product.gtin.as_deref(), product.mpn.as_deref());
        if let (Some(cents), Some(currency)) = (product.amount_cents, &product.currency) {
            ld.insert(
                "offers".into(),
                offer(
                    page.url,
                    cents,
                    product.compare_at_cents,
                    currency,
                    !product.is_sold_out(None),
                    &Delivery::of(product, page.terms),
                ),
            );
        }
    } else {
        ld.insert("@type".into(), json!("ProductGroup"));
        ld.insert("productGroupID".into(), json!(product.id));
        let varies_by: Vec<String> = product
            .options
            .iter()
            .filter_map(|option| option.kind.as_deref())
            .map(|kind| format!("https://schema.org/{kind}"))
            .collect();
        if !varies_by.is_empty() {
            ld.insert("variesBy".into(), json!(varies_by));
        }
        let variants: Vec<Value> = product
            .variants
            .iter()
            .map(|variant| variant_ld(product, variant, &variant_url(variant), page.terms))
            .collect();
        ld.insert("hasVariant".into(), json!(variants));
    }
    if let Some(rating) = product.rating() {
        ld.insert(
            "aggregateRating".into(),
            json!({
                "@type": "AggregateRating",
                "ratingValue": format!("{rating:.1}"),
                "bestRating": "5",
                "reviewCount": product.review_count,
            }),
        );
    }
    if !page.reviews.is_empty() {
        let reviews: Vec<Value> = page.reviews.iter().map(review_ld).collect();
        ld.insert("review".into(), json!(reviews));
    }
    Value::Object(ld)
}

/// The numbers a product or a variant is known under, where it has them.
fn identifiers(ld: &mut Map<String, Value>, gtin: Option<&str>, mpn: Option<&str>) {
    if let Some(gtin) = gtin {
        ld.insert("gtin".into(), json!(gtin));
    }
    if let Some(mpn) = mpn {
        ld.insert("mpn".into(), json!(mpn));
    }
}

/// One variant of a group: named after the product and what it takes,
/// with its own code, numbers, picture and offer. What it takes for an
/// option search engines know (a colour, a size) goes under that name, the
/// rest as free properties.
fn variant_ld(product: &Product, variant: &Variant, url: &str, terms: &OfferTerms) -> Value {
    let label = product.variant_label(variant);
    let mut ld = Map::new();
    ld.insert("@type".into(), json!("Product"));
    ld.insert(
        "name".into(),
        json!(if label.is_empty() {
            product.name.clone()
        } else {
            format!("{} – {label}", product.name)
        }),
    );
    ld.insert(
        "sku".into(),
        json!(variant.sku.as_deref().unwrap_or(&variant.id)),
    );
    let image = variant
        .image_id
        .as_deref()
        .and_then(|id| product.images.iter().find(|image| image.id == id))
        .or(product.images.first());
    if let Some(image) = image {
        ld.insert("image".into(), json!(image.url));
    }
    identifiers(&mut ld, variant.gtin.as_deref(), variant.mpn.as_deref());
    let mut properties: Vec<Value> = Vec::new();
    for selection in &variant.selections {
        let Some(option) = product
            .options
            .iter()
            .find(|option| option.id == selection.option_id)
        else {
            continue;
        };
        let value = option.display_value(&selection.value);
        match &option.kind {
            Some(kind) => {
                ld.insert(kind.clone(), json!(value));
            }
            None => properties.push(json!({
                "@type": "PropertyValue",
                "name": option.display_name(),
                "value": value,
            })),
        }
    }
    if !properties.is_empty() {
        ld.insert("additionalProperty".into(), json!(properties));
    }
    ld.insert(
        "offers".into(),
        offer(
            url,
            variant.amount_cents,
            variant.compare_at_cents,
            &variant.currency,
            !product.is_sold_out(Some(&variant.id)),
            &Delivery::of(product, terms),
        ),
    );
    Value::Object(ld)
}

/// What an offer says of delivery and returns: the shop's terms, and how
/// long the product takes to arrive.
struct Delivery<'a> {
    terms: &'a OfferTerms,
    /// The product's delivery time, or the shop's default, in days.
    days: Option<(u16, u16)>,
}

impl<'a> Delivery<'a> {
    fn of(product: &Product, terms: &'a OfferTerms) -> Self {
        Self {
            terms,
            days: product.delivery_days.map(|days| (days.min, days.max)),
        }
    }

    /// One `OfferShippingDetails` per shipping method priced in `currency`
    /// that names its countries: what it costs for goods worth `cents`
    /// (nothing from its free threshold), where it goes, and how long it
    /// takes when the shop says. A method that ships everywhere names no
    /// country, and search engines want one: it is left out.
    fn shipping_details(&self, cents: i64, currency: &str) -> Vec<Value> {
        self.terms
            .shipping
            .iter()
            .filter(|method| method.currency == currency && !method.countries.is_empty())
            .map(|method| {
                let rate = method.price_for(cents.max(0) as u64);
                let mut details = json!({
                    "@type": "OfferShippingDetails",
                    "shippingRate": {
                        "@type": "MonetaryAmount",
                        "value": decimal(rate as i64),
                        "currency": currency,
                    },
                    "shippingDestination": method
                        .countries
                        .iter()
                        .map(|country| json!({ "@type": "DefinedRegion", "addressCountry": country }))
                        .collect::<Vec<_>>(),
                });
                // The shop's delivery time runs from the order to the
                // door: no handling counted apart from it.
                if let Some((min, max)) = self.days {
                    details["deliveryTime"] = json!({
                        "@type": "ShippingDeliveryTime",
                        "handlingTime": {
                            "@type": "QuantitativeValue",
                            "minValue": 0,
                            "maxValue": 0,
                            "unitCode": "DAY",
                        },
                        "transitTime": {
                            "@type": "QuantitativeValue",
                            "minValue": min,
                            "maxValue": max,
                            "unitCode": "DAY",
                        },
                    });
                }
                details
            })
            .collect()
    }

    /// The shop's return terms as a `MerchantReturnPolicy`, for the
    /// countries its shipping methods name (a policy must say where it
    /// holds): `None` when the shop states no terms or names no country.
    fn return_policy(&self) -> Option<Value> {
        let terms = self.terms.returns?;
        let mut countries: Vec<&str> = self
            .terms
            .shipping
            .iter()
            .flat_map(|method| method.countries.iter().map(String::as_str))
            .collect();
        countries.sort_unstable();
        countries.dedup();
        if countries.is_empty() {
            return None;
        }
        Some(json!({
            "@type": "MerchantReturnPolicy",
            "applicableCountry": countries,
            "returnPolicyCategory": "https://schema.org/MerchantReturnFiniteReturnWindow",
            "merchantReturnDays": terms.days,
            "returnMethod": "https://schema.org/ReturnByMail",
            "returnFees": if terms.free {
                "https://schema.org/FreeReturn"
            } else {
                "https://schema.org/ReturnFeesCustomerResponsibility"
            },
        }))
    }
}

/// What a shopper pays, the price it is down from when the page strikes
/// one through, and what the shop says of delivery and returns.
fn offer(
    url: &str,
    cents: i64,
    compare_at: Option<i64>,
    currency: &str,
    in_stock: bool,
    delivery: &Delivery<'_>,
) -> Value {
    let mut offer = json!({
        "@type": "Offer",
        "url": url,
        "price": decimal(cents),
        "priceCurrency": currency,
        "itemCondition": "https://schema.org/NewCondition",
        "availability": if in_stock {
            "https://schema.org/InStock"
        } else {
            "https://schema.org/OutOfStock"
        },
    });
    if let Some(compare_at) = compare_at {
        offer["priceSpecification"] = json!({
            "@type": "UnitPriceSpecification",
            "priceType": "https://schema.org/StrikethroughPrice",
            "price": decimal(compare_at),
            "priceCurrency": currency,
        });
    }
    let shipping = delivery.shipping_details(cents, currency);
    if !shipping.is_empty() {
        offer["shippingDetails"] = json!(shipping);
    }
    if let Some(policy) = delivery.return_policy() {
        offer["hasMerchantReturnPolicy"] = policy;
    }
    offer
}

fn review_ld(review: &Review) -> Value {
    let mut ld = Map::new();
    ld.insert("@type".into(), json!("Review"));
    ld.insert(
        "author".into(),
        json!({ "@type": "Person", "name": review.author_name }),
    );
    if let Ok(at) = u64::try_from(review.published_at) {
        ld.insert("datePublished".into(), json!(format_date(at)));
    }
    ld.insert(
        "reviewRating".into(),
        json!({ "@type": "Rating", "ratingValue": review.rating, "bestRating": 5 }),
    );
    if let Some(title) = &review.title {
        ld.insert("name".into(), json!(title));
    }
    ld.insert("reviewBody".into(), json!(review.body));
    Value::Object(ld)
}

/// A breadcrumb as schema.org data: `(name, whole address)` pairs from the
/// home down to the page itself.
pub fn breadcrumb_ld<'a>(crumbs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
    let items: Vec<Value> = crumbs
        .into_iter()
        .enumerate()
        .map(|(position, (name, url))| {
            json!({
                "@type": "ListItem",
                "position": position + 1,
                "name": name,
                "item": url,
            })
        })
        .collect();
    json!({
        "@context": "https://schema.org",
        "@type": "BreadcrumbList",
        "itemListElement": items,
    })
}

/// The shop as an organization, for its home page. No rating: a shop's
/// rating of itself is self-serving, and search engines ignore it.
pub fn organization_ld(name: &str, url: &str, email: Option<&str>) -> Value {
    let mut ld = Map::new();
    ld.insert("@context".into(), json!("https://schema.org"));
    ld.insert("@type".into(), json!("Organization"));
    ld.insert("name".into(), json!(name));
    ld.insert("url".into(), json!(url));
    if let Some(email) = email {
        ld.insert("email".into(), json!(email));
    }
    Value::Object(ld)
}

/// The shop as a site, for its home page: the name a search result shows
/// above its address.
pub fn website_ld(name: &str, url: &str) -> Value {
    json!({
        "@context": "https://schema.org",
        "@type": "WebSite",
        "name": name,
        "url": url,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::query::{Image, ProductOption, Selection};
    use crate::terms::{ReturnTerms, ShippingMethod};

    fn image(id: &str) -> Image {
        Image {
            id: id.to_owned(),
            alt: String::new(),
            width: 1200,
            height: 1200,
            thumb_width: 400,
            thumb_height: 400,
            url: format!("https://media.example/1200/{id}.webp"),
            thumb_url: format!("https://media.example/400/{id}.webp"),
        }
    }

    fn product() -> Product {
        Product {
            id: "01MUG".into(),
            slug: "mug".into(),
            name: "Mug".into(),
            description: "Holds tea.".into(),
            amount_cents: Some(1990),
            currency: Some("EUR".into()),
            updated_at: 1,
            images: vec![image("a"), image("b")],
            options: Vec::new(),
            variants: Vec::new(),
            attributes: Vec::new(),
            min_amount_cents: Some(1990),
            max_amount_cents: Some(1990),
            review_count: 0,
            rating_x100: 0,
            stock: BTreeMap::new(),
            in_stock: true,
            delivery_days: None,
            brand: None,
            gtin: None,
            mpn: None,
            compare_at_cents: None,
        }
    }

    /// A shop that says nothing of delivery nor returns.
    static NO_TERMS: OfferTerms = OfferTerms {
        shipping: Vec::new(),
        returns: None,
    };

    fn page<'a>(reviews: &'a [Review]) -> ProductPageLd<'a> {
        ProductPageLd {
            url: "https://shop.example/p/mug",
            description: "Holds tea.",
            reviews,
            terms: &NO_TERMS,
        }
    }

    fn method(name: &str, countries: &[&str], free_above_cents: Option<u64>) -> ShippingMethod {
        ShippingMethod {
            id: name.to_owned(),
            name: name.to_owned(),
            price_cents: 490,
            currency: "EUR".into(),
            countries: countries.iter().map(|c| c.to_string()).collect(),
            free_above_cents,
        }
    }

    #[test]
    fn an_offer_says_what_delivery_costs_and_how_returns_go() {
        let terms = OfferTerms {
            shipping: vec![
                method("Colissimo", &["FR", "BE"], Some(5000)),
                // Free from a threshold the mug reaches.
                method("Relais", &["FR"], Some(1500)),
                // Ships everywhere: no country to name, left out.
                method("Monde", &[], None),
                // Priced in another currency than the offer.
                ShippingMethod {
                    currency: "USD".into(),
                    ..method("US Mail", &["US"], None)
                },
            ],
            returns: Some(ReturnTerms {
                days: 30,
                free: false,
            }),
        };
        let mut mug = product();
        mug.delivery_days = Some(timada_catalog::DeliveryDays { min: 2, max: 4 });
        let ld = product_ld(
            &mug,
            &ProductPageLd {
                terms: &terms,
                ..page(&[])
            },
            |_| unreachable!(),
        );
        let time = json!({
            "@type": "ShippingDeliveryTime",
            "handlingTime": {
                "@type": "QuantitativeValue", "minValue": 0, "maxValue": 0, "unitCode": "DAY",
            },
            "transitTime": {
                "@type": "QuantitativeValue", "minValue": 2, "maxValue": 4, "unitCode": "DAY",
            },
        });
        assert_eq!(
            ld["offers"]["shippingDetails"],
            json!([
                {
                    "@type": "OfferShippingDetails",
                    "shippingRate": { "@type": "MonetaryAmount", "value": "4.90", "currency": "EUR" },
                    "shippingDestination": [
                        { "@type": "DefinedRegion", "addressCountry": "FR" },
                        { "@type": "DefinedRegion", "addressCountry": "BE" },
                    ],
                    "deliveryTime": time,
                },
                {
                    "@type": "OfferShippingDetails",
                    "shippingRate": { "@type": "MonetaryAmount", "value": "0.00", "currency": "EUR" },
                    "shippingDestination": [{ "@type": "DefinedRegion", "addressCountry": "FR" }],
                    "deliveryTime": time,
                },
            ])
        );
        assert_eq!(
            ld["offers"]["hasMerchantReturnPolicy"],
            json!({
                "@type": "MerchantReturnPolicy",
                "applicableCountry": ["BE", "FR", "US"],
                "returnPolicyCategory": "https://schema.org/MerchantReturnFiniteReturnWindow",
                "merchantReturnDays": 30,
                "returnMethod": "https://schema.org/ReturnByMail",
                "returnFees": "https://schema.org/ReturnFeesCustomerResponsibility",
            })
        );

        // No delivery time said: none written. Free returns say so. A shop
        // whose methods name no country has nowhere to state its returns.
        mug.delivery_days = None;
        let free = OfferTerms {
            shipping: vec![method("Colissimo", &["FR"], None)],
            returns: Some(ReturnTerms {
                days: 14,
                free: true,
            }),
        };
        let ld = product_ld(
            &mug,
            &ProductPageLd {
                terms: &free,
                ..page(&[])
            },
            |_| unreachable!(),
        );
        assert!(
            ld["offers"]["shippingDetails"][0]
                .get("deliveryTime")
                .is_none()
        );
        assert_eq!(
            ld["offers"]["hasMerchantReturnPolicy"]["returnFees"],
            "https://schema.org/FreeReturn"
        );
        let nowhere = OfferTerms {
            shipping: vec![method("Monde", &[], None)],
            returns: free.returns,
        };
        let ld = product_ld(
            &mug,
            &ProductPageLd {
                terms: &nowhere,
                ..page(&[])
            },
            |_| unreachable!(),
        );
        assert!(ld["offers"].get("shippingDetails").is_none());
        assert!(ld["offers"].get("hasMerchantReturnPolicy").is_none());
    }

    #[test]
    fn a_product_sold_as_is_carries_one_offer() {
        let ld = product_ld(&product(), &page(&[]), |_| unreachable!());
        assert_eq!(
            ld,
            json!({
                "@context": "https://schema.org",
                "@type": "Product",
                "name": "Mug",
                "url": "https://shop.example/p/mug",
                "description": "Holds tea.",
                "image": [
                    "https://media.example/1200/a.webp",
                    "https://media.example/1200/b.webp",
                ],
                "sku": "01MUG",
                "offers": {
                    "@type": "Offer",
                    "url": "https://shop.example/p/mug",
                    "price": "19.90",
                    "priceCurrency": "EUR",
                    "itemCondition": "https://schema.org/NewCondition",
                    "availability": "https://schema.org/InStock",
                },
            })
        );

        // Counted and none left: out of stock. No price: no offer at all.
        let mut sold_out = product();
        sold_out.stock.insert(String::new(), 0);
        let ld = product_ld(&sold_out, &page(&[]), |_| unreachable!());
        assert_eq!(
            ld["offers"]["availability"],
            "https://schema.org/OutOfStock"
        );
        let mut unpriced = product();
        unpriced.amount_cents = None;
        let ld = product_ld(&unpriced, &page(&[]), |_| unreachable!());
        assert!(ld.get("offers").is_none());
    }

    #[test]
    fn a_product_with_variants_is_a_group_with_an_offer_each() {
        let mut collar = product();
        collar.options = vec![ProductOption {
            id: "o1".into(),
            name: "Color".into(),
            values: vec!["red".into(), "blue".into()],
            label: Some("Couleur".into()),
            value_labels: BTreeMap::from([("red".to_owned(), "Rouge".to_owned())]),
            kind: None,
        }];
        let variant =
            |id: &str, value: &str, cents, sku: Option<&str>, image: Option<&str>| Variant {
                id: id.to_owned(),
                selections: vec![Selection {
                    option_id: "o1".into(),
                    value: value.to_owned(),
                }],
                sku: sku.map(str::to_owned),
                amount_cents: cents,
                currency: "EUR".into(),
                image_id: image.map(str::to_owned),
                gtin: None,
                mpn: None,
                compare_at_cents: None,
            };
        collar.variants = vec![
            variant("v1", "red", 990, Some("MUG-R"), Some("b")),
            variant("v2", "blue", 1290, None, None),
        ];
        collar.stock.insert("v2".into(), 0);
        let ld = product_ld(&collar, &page(&[]), |variant| {
            format!("https://shop.example/p/mug?v={}", variant.id)
        });
        assert_eq!(ld["@type"], "ProductGroup");
        assert_eq!(ld["productGroupID"], "01MUG");
        assert!(ld.get("offers").is_none() && ld.get("sku").is_none());
        assert_eq!(
            ld["hasVariant"],
            json!([
                {
                    "@type": "Product",
                    "name": "Mug – Rouge",
                    "sku": "MUG-R",
                    "image": "https://media.example/1200/b.webp",
                    "additionalProperty": [
                        { "@type": "PropertyValue", "name": "Couleur", "value": "Rouge" },
                    ],
                    "offers": {
                        "@type": "Offer",
                        "url": "https://shop.example/p/mug?v=v1",
                        "price": "9.90",
                        "priceCurrency": "EUR",
                        "itemCondition": "https://schema.org/NewCondition",
                        "availability": "https://schema.org/InStock",
                    },
                },
                {
                    "@type": "Product",
                    "name": "Mug – blue",
                    "sku": "v2",
                    "image": "https://media.example/1200/a.webp",
                    "additionalProperty": [
                        { "@type": "PropertyValue", "name": "Couleur", "value": "blue" },
                    ],
                    "offers": {
                        "@type": "Offer",
                        "url": "https://shop.example/p/mug?v=v2",
                        "price": "12.90",
                        "priceCurrency": "EUR",
                        "itemCondition": "https://schema.org/NewCondition",
                        "availability": "https://schema.org/OutOfStock",
                    },
                },
            ])
        );
    }

    #[test]
    fn a_brand_and_the_numbers_are_told_where_the_product_has_them() {
        let mut mug = product();
        let ld = product_ld(&mug, &page(&[]), |_| unreachable!());
        assert!(ld.get("brand").is_none() && ld.get("gtin").is_none() && ld.get("mpn").is_none());
        mug.brand = Some("Acme".into());
        mug.gtin = Some("4006381333931".into());
        mug.mpn = Some("MUG-01".into());
        let ld = product_ld(&mug, &page(&[]), |_| unreachable!());
        assert_eq!(ld["brand"], json!({ "@type": "Brand", "name": "Acme" }));
        assert_eq!(ld["gtin"], "4006381333931");
        assert!(ld["offers"].get("priceSpecification").is_none());
        mug.compare_at_cents = Some(2490);
        let ld = product_ld(&mug, &page(&[]), |_| unreachable!());
        assert_eq!(ld["offers"]["price"], "19.90");
        assert_eq!(ld["offers"]["priceSpecification"]["price"], "24.90");
        assert_eq!(ld["mpn"], "MUG-01");
    }

    #[test]
    fn a_group_says_what_it_varies_by_and_a_variant_what_it_takes() {
        let mut collar = product();
        collar.brand = Some("Acme".into());
        // The product's own numbers are not a variant's.
        collar.gtin = Some("4006381333931".into());
        let option = |id: &str, name: &str, kind: Option<&str>| ProductOption {
            id: id.to_owned(),
            name: name.to_owned(),
            values: vec!["a".into()],
            label: None,
            value_labels: BTreeMap::from([("a".to_owned(), "Rouge".to_owned())]),
            kind: kind.map(str::to_owned),
        };
        collar.options = vec![
            option("o1", "Color", Some("color")),
            option("o2", "Size", Some("size")),
            option("o3", "Clasp", None),
        ];
        collar.variants = vec![Variant {
            id: "v1".into(),
            selections: ["o1", "o2", "o3"]
                .map(|option_id| Selection {
                    option_id: option_id.into(),
                    value: "a".into(),
                })
                .to_vec(),
            sku: None,
            amount_cents: 990,
            currency: "EUR".into(),
            image_id: None,
            gtin: Some("036000291452".into()),
            mpn: Some("C-R".into()),
            compare_at_cents: Some(1290),
        }];
        let ld = product_ld(&collar, &page(&[]), |_| "https://shop.example/p/c".into());
        assert_eq!(ld["brand"]["name"], "Acme");
        assert!(ld.get("gtin").is_none());
        assert_eq!(
            ld["variesBy"],
            json!(["https://schema.org/color", "https://schema.org/size"])
        );
        let variant = &ld["hasVariant"][0];
        assert_eq!(variant["gtin"], "036000291452");
        assert_eq!(variant["offers"]["price"], "9.90");
        assert_eq!(
            variant["offers"]["priceSpecification"],
            json!({
                "@type": "UnitPriceSpecification",
                "priceType": "https://schema.org/StrikethroughPrice",
                "price": "12.90",
                "priceCurrency": "EUR",
            })
        );
        assert_eq!(variant["mpn"], "C-R");
        assert_eq!(variant["color"], "Rouge");
        assert_eq!(variant["size"], "Rouge");
        assert_eq!(
            variant["additionalProperty"],
            json!([{ "@type": "PropertyValue", "name": "Clasp", "value": "Rouge" }])
        );
    }

    #[test]
    fn a_rating_and_reviews_are_there_only_when_the_product_has_some() {
        let ld = product_ld(&product(), &page(&[]), |_| unreachable!());
        assert!(ld.get("aggregateRating").is_none() && ld.get("review").is_none());

        let mut rated = product();
        rated.review_count = 3;
        rated.rating_x100 = 435;
        let reviews = [Review {
            id: "r1".into(),
            product_id: "01MUG".into(),
            rating: 5,
            title: Some("Parfait".into()),
            body: "Mon chat l'adore.".into(),
            author_name: "Alice".into(),
            variant_label: None,
            photos: Vec::new(),
            published_at: 1_709_164_800,
            reply: None,
            replied_at: None,
        }];
        let ld = product_ld(&rated, &page(&reviews), |_| unreachable!());
        assert_eq!(
            ld["aggregateRating"],
            json!({
                "@type": "AggregateRating",
                "ratingValue": "4.4",
                "bestRating": "5",
                "reviewCount": 3,
            })
        );
        assert_eq!(
            ld["review"],
            json!([{
                "@type": "Review",
                "author": { "@type": "Person", "name": "Alice" },
                "datePublished": "2024-02-29",
                "reviewRating": { "@type": "Rating", "ratingValue": 5, "bestRating": 5 },
                "name": "Parfait",
                "reviewBody": "Mon chat l'adore.",
            }])
        );
    }

    #[test]
    fn a_breadcrumb_numbers_its_steps_from_the_home() {
        let ld = breadcrumb_ld([
            ("Accueil", "https://shop.example/"),
            ("Tapis", "https://shop.example/c/tapis"),
        ]);
        assert_eq!(ld["@type"], "BreadcrumbList");
        assert_eq!(
            ld["itemListElement"],
            json!([
                { "@type": "ListItem", "position": 1, "name": "Accueil", "item": "https://shop.example/" },
                { "@type": "ListItem", "position": 2, "name": "Tapis", "item": "https://shop.example/c/tapis" },
            ])
        );
    }

    #[test]
    fn the_home_names_the_shop_without_rating_it() {
        let org = organization_ld("Acme", "https://shop.example/", Some("hello@shop.example"));
        assert_eq!(
            org,
            json!({
                "@context": "https://schema.org",
                "@type": "Organization",
                "name": "Acme",
                "url": "https://shop.example/",
                "email": "hello@shop.example",
            })
        );
        assert_eq!(
            website_ld("Acme", "https://shop.example/")["@type"],
            "WebSite"
        );
    }

    #[test]
    fn json_in_a_script_cannot_close_it() {
        let text = json_ld(&json!({ "name": "</script><!-- a & b" }));
        assert_eq!(
            text,
            r#"{"name":"\u003c/script\u003e\u003c!-- a \u0026 b"}"#
        );
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap()["name"],
            "</script><!-- a & b"
        );
    }

    #[test]
    fn a_card_is_a_products_when_it_has_a_price_and_large_when_it_has_a_picture() {
        let plain = SocialCard {
            title: "Acme",
            url: "https://shop.example/",
            site_name: "Acme",
            ..Default::default()
        };
        let keys = |card: &SocialCard<'_>| -> Vec<(&'static str, String)> {
            card.tags()
                .into_iter()
                .map(|tag| (tag.key, tag.content))
                .collect()
        };
        assert_eq!(
            keys(&plain),
            [
                ("og:type", "website".to_owned()),
                ("og:title", "Acme".to_owned()),
                ("og:url", "https://shop.example/".to_owned()),
                ("og:site_name", "Acme".to_owned()),
                ("twitter:card", "summary".to_owned()),
                ("twitter:title", "Acme".to_owned()),
            ]
        );
        let product = SocialCard {
            title: "Mug",
            description: "Holds tea.",
            url: "https://shop.example/p/mug",
            site_name: "Acme",
            image: Some("https://media.example/1200/a.webp"),
            price: Some((1990, "EUR")),
        };
        let tags = product.tags();
        let content = |key: &str| {
            tags.iter()
                .find(|tag| tag.key == key)
                .map(|tag| tag.content.as_str())
        };
        assert_eq!(content("og:type"), Some("product"));
        assert_eq!(content("og:description"), Some("Holds tea."));
        assert_eq!(content("product:price:amount"), Some("19.90"));
        assert_eq!(content("product:price:currency"), Some("EUR"));
        assert_eq!(content("twitter:card"), Some("summary_large_image"));
        assert_eq!(
            content("twitter:image"),
            Some("https://media.example/1200/a.webp")
        );
        assert!(
            tags.iter()
                .all(|tag| tag.property == tag.key.starts_with("og:")
                    || tag.key.starts_with("product:") && tag.property)
        );
    }
}
