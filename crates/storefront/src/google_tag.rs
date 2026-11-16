//! The shop's Google tag: what the merchant set up in the back office
//! (`timada-pages`' `GoogleTag`: the Analytics and Google Ads ids, the
//! conversion labels), copied into `storefront_google_tag` by the catalog
//! subscription, and the scripts a page writes from it.
//!
//! [`google_tag_head`] gives the tag itself, or nothing while the
//! visitor's answer to the cookie banner holds it back
//! ([`ConsentMode::loads`]); it goes in the `<head>` after
//! [`consent_script`](crate::consent_script). [`purchase_events`],
//! [`add_to_cart_events`] and [`begin_checkout_events`] give the events, as
//! the arguments of one `gtag(…)` call each: the Analytics event when the
//! shop has an Analytics id, and the Google Ads conversion when it has a
//! label for it.
//!
//! A page writes its events as data ([`tag_events_json`], in a
//! `<script type="application/json" data-tag-events>`) and
//! [`TAG_EVENTS_SCRIPT`] after it, which sends them. A script that fetches
//! a page to show a part of it (a cart drawer) hands the fetched document
//! to `timadaTag(…)`, so the events that page carried are sent all the same.
//!
//! The value of a sale is the order's total, as the shopper paid it.

use evento::{Executor, metadata::RawEvent, subscription::Context};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use timada_core::db::{Db, FromRow};
use timada_orders::{OrderState, ShippingAddress};
pub use timada_pages::GoogleTagSettings;
use timada_pages::aggregator::GoogleTag;
use timada_pages::google_tag_projection;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::consent::{Consent, ConsentMode};
use crate::context::store;
use crate::head::json_ld;

#[derive(FromRow)]
struct TagRow {
    analytics_id: Option<String>,
    ads_id: Option<String>,
    purchase_label: Option<String>,
    add_to_cart_label: Option<String>,
    begin_checkout_label: Option<String>,
    enhanced_conversions: i64,
}

/// The tag as the copy holds it; `None` for a shop that set none up.
pub async fn google_tag_settings(db: &Db) -> anyhow::Result<Option<GoogleTagSettings>> {
    let row: Option<TagRow> = db
        .fetch_optional(
            "SELECT analytics_id, ads_id, purchase_label, add_to_cart_label,
                begin_checkout_label, enhanced_conversions
             FROM storefront_google_tag WHERE id = 1",
            (),
        )
        .await?;
    Ok(row
        .map(|row| GoogleTagSettings {
            analytics_id: row.analytics_id,
            ads_id: row.ads_id,
            purchase_label: row.purchase_label,
            add_to_cart_label: row.add_to_cart_label,
            begin_checkout_label: row.begin_checkout_label,
            enhanced_conversions: row.enhanced_conversions != 0,
        })
        .filter(GoogleTagSettings::is_set))
}

/// The shop's Google tag, when it set one up; see the module docs.
pub async fn google_tag(cx: &Cx) -> Result<Option<GoogleTagSettings>> {
    google_tag_settings(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// Rewrites the one row from the stream.
#[evento::subscription_all]
pub(crate) async fn on_google_tag_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<GoogleTag>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let tag = google_tag_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .map(|state| state.settings)
        .unwrap_or_default();
    db.execute(
        "INSERT INTO storefront_google_tag (id, analytics_id, ads_id, purchase_label,
            add_to_cart_label, begin_checkout_label, enhanced_conversions)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (id) DO UPDATE SET
            analytics_id = excluded.analytics_id, ads_id = excluded.ads_id,
            purchase_label = excluded.purchase_label,
            add_to_cart_label = excluded.add_to_cart_label,
            begin_checkout_label = excluded.begin_checkout_label,
            enhanced_conversions = excluded.enhanced_conversions",
        (
            tag.analytics_id.as_deref(),
            tag.ads_id.as_deref(),
            tag.purchase_label.as_deref(),
            tag.add_to_cart_label.as_deref(),
            tag.begin_checkout_label.as_deref(),
            i64::from(tag.enhanced_conversions),
        ),
    )
    .await?;
    Ok(())
}

/// The tag in a page's `<head>`: Google's script, loaded `async` from
/// `src`, and the inline `script` that starts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagHead {
    pub src: String,
    pub script: String,
}

/// The tag for a page, or nothing: a shop with no tag, or a visitor whose
/// answer holds it back in the shop's consent mode. The page writes
/// [`consent_script`](crate::consent_script) before it, which defines
/// `gtag` and sets the signals the tag starts from.
pub fn google_tag_head(
    tag: &GoogleTagSettings,
    mode: ConsentMode,
    answer: Option<Consent>,
) -> Option<TagHead> {
    if !mode.loads(answer) {
        return None;
    }
    // One script serves every id configured after it.
    let first = tag.analytics_id.as_deref().or(tag.ads_id.as_deref())?;
    let mut script = String::from("gtag('js',new Date());");
    if let Some(id) = &tag.analytics_id {
        script.push_str(&format!("gtag('config','{id}');"));
    }
    if let Some(id) = &tag.ads_id {
        if tag.enhanced_conversions {
            script.push_str(&format!(
                "gtag('config','{id}',{{allow_enhanced_conversions:true}});"
            ));
        } else {
            script.push_str(&format!("gtag('config','{id}');"));
        }
    }
    Some(TagHead {
        src: format!("https://www.googletagmanager.com/gtag/js?id={first}"),
        script,
    })
}

/// One thing sold, as an event lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TagItem {
    /// The variant's SKU when it has one, else the product's id: what the
    /// shopping feed calls it too.
    pub id: String,
    pub name: String,
    /// The variant's values (`Beige / Large`).
    pub variant: Option<String>,
    /// The price of one, in minor units.
    pub unit_cents: u64,
    pub quantity: u32,
}

/// An amount in minor units as the number Google reads (`19.9`).
fn amount(cents: u64) -> Value {
    json!(cents as f64 / 100.0)
}

fn items(items: &[TagItem]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| {
                let mut object = Map::new();
                object.insert("item_id".into(), json!(item.id));
                object.insert("item_name".into(), json!(item.name));
                if let Some(variant) = &item.variant {
                    object.insert("item_variant".into(), json!(variant));
                }
                object.insert("price".into(), amount(item.unit_cents));
                object.insert("quantity".into(), json!(item.quantity));
                Value::Object(object)
            })
            .collect(),
    )
}

fn value_of(items: &[TagItem]) -> u64 {
    items
        .iter()
        .map(|item| item.unit_cents * u64::from(item.quantity))
        .sum()
}

/// The arguments of `gtag('event', name, params)`.
fn event(name: &str, params: Value) -> Value {
    json!(["event", name, params])
}

/// The events of a page as the text of a
/// `<script type="application/json" data-tag-events>`: JSON in which nothing
/// can close the script.
pub fn tag_events_json(events: &[Value]) -> String {
    json_ld(&Value::Array(events.to_vec()))
}

/// Sends the events a document carries in its `data-tag-events` scripts,
/// once each, and leaves `timadaTag(document)` for a script that fetched
/// another page. Written after the events of the page itself, on a page
/// that has the tag.
pub const TAG_EVENTS_SCRIPT: &str = "window.timadaTag=function(d){d.querySelectorAll('script[data-tag-events]').forEach(function(s){if(s.dataset.sent)return;s.dataset.sent='1';try{JSON.parse(s.textContent).forEach(function(c){gtag.apply(null,c)})}catch(e){}})};timadaTag(document);";

/// The Analytics event `name` (when the shop has an Analytics id), then
/// the Google Ads conversion under `label` (when it has one), with the same
/// value and currency.
fn events(
    tag: &GoogleTagSettings,
    name: &str,
    label: Option<&str>,
    mut params: Map<String, Value>,
    transaction_id: Option<&str>,
) -> Vec<Value> {
    let mut sent = Vec::new();
    if let (Some(ads_id), Some(label)) = (&tag.ads_id, label) {
        let mut conversion = Map::new();
        conversion.insert("send_to".into(), json!(format!("{ads_id}/{label}")));
        for key in ["value", "currency"] {
            if let Some(value) = params.get(key) {
                conversion.insert(key.into(), value.clone());
            }
        }
        if let Some(id) = transaction_id {
            conversion.insert("transaction_id".into(), json!(id));
        }
        sent.push(event("conversion", Value::Object(conversion)));
    }
    if let Some(analytics_id) = &tag.analytics_id {
        params.insert("send_to".into(), json!(analytics_id));
        sent.insert(0, event(name, Value::Object(params)));
    }
    sent
}

/// What a page sends once something went into the cart: the secondary
/// conversion `add_to_cart`. Empty when the shop counts it nowhere.
pub fn add_to_cart_events(
    tag: &GoogleTagSettings,
    currency: &str,
    added: &[TagItem],
) -> Vec<Value> {
    let mut params = Map::new();
    params.insert("currency".into(), json!(currency));
    params.insert("value".into(), amount(value_of(added)));
    params.insert("items".into(), items(added));
    events(
        tag,
        "add_to_cart",
        tag.add_to_cart_label.as_deref(),
        params,
        None,
    )
}

/// What the checkout's page sends: the secondary conversion
/// `begin_checkout`, valued at what the cart's goods cost.
pub fn begin_checkout_events(
    tag: &GoogleTagSettings,
    currency: &str,
    value_cents: u64,
    cart: &[TagItem],
) -> Vec<Value> {
    let mut params = Map::new();
    params.insert("currency".into(), json!(currency));
    params.insert("value".into(), amount(value_cents));
    params.insert("items".into(), items(cart));
    events(
        tag,
        "begin_checkout",
        tag.begin_checkout_label.as_deref(),
        params,
        None,
    )
}

/// The lines of an order, as an event lists them.
pub fn order_items(order: &OrderState) -> Vec<TagItem> {
    order
        .lines
        .iter()
        .map(|line| TagItem {
            id: line.sku.clone().unwrap_or_else(|| line.product_id.clone()),
            name: line.name.clone(),
            variant: line.variant_label.clone(),
            unit_cents: line.unit_amount_cents,
            quantity: line.quantity,
        })
        .collect()
}

/// What the order's confirmation page sends, once per order (the host
/// sees to that): the primary conversion `purchase`, with the order's
/// total as its value, its currency, its lines, and its id as the
/// transaction id, which lets Google drop a repeat.
///
/// With enhanced conversions on and advertising allowed (`answer`), the
/// buyer's data goes first: see [`user_data`].
pub fn purchase_events(
    tag: &GoogleTagSettings,
    order: &OrderState,
    answer: Option<Consent>,
) -> Vec<Value> {
    let mut params = Map::new();
    params.insert("transaction_id".into(), json!(order.id));
    params.insert("value".into(), amount(order.total_cents));
    params.insert("currency".into(), json!(order.currency));
    if let Some(shipping) = &order.shipping_charge {
        params.insert("shipping".into(), amount(shipping.amount_cents));
    }
    if let Some(discount) = &order.discount {
        params.insert("coupon".into(), json!(discount.code));
    }
    params.insert("items".into(), items(&order_items(order)));
    let mut sale = events(
        tag,
        "purchase",
        tag.purchase_label.as_deref(),
        params,
        Some(&order.id),
    );
    let enhanced =
        tag.enhanced_conversions && tag.ads_id.is_some() && answer.is_some_and(|c| c.ads);
    if enhanced && !sale.is_empty() {
        sale.insert(
            0,
            json!(["set", "user_data", user_data(&order.email, &order.shipping)]),
        );
    }
    sale
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// An e-mail address as Google wants it before hashing: trimmed,
/// lowercase, and without the dots before the `@` of a Gmail address.
fn normalized_email(email: &str) -> String {
    let email = email.trim().to_lowercase();
    match email.rsplit_once('@') {
        Some((name, domain @ ("gmail.com" | "googlemail.com"))) => {
            format!("{}@{domain}", name.replace('.', ""))
        }
        _ => email,
    }
}

/// A phone number in E.164 (`+33612345678`), when it was written with its
/// country code: 11 to 15 digits after the plus.
fn e164(phone: &str) -> Option<String> {
    let phone = phone.trim();
    let digits: String = phone
        .strip_prefix('+')?
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    (11..=15)
        .contains(&digits.len())
        .then(|| format!("+{digits}"))
}

/// A buyer as Enhanced Conversions takes them: the e-mail address, the
/// phone and the name hashed (hex SHA-256 of the normalized value), the
/// street, the city, the postal code and the country as they are. The name
/// the parcel is for is split at its first space.
pub fn user_data(email: &str, to: &ShippingAddress) -> Value {
    let mut data = Map::new();
    data.insert(
        "sha256_email_address".into(),
        json!(sha256(&normalized_email(email))),
    );
    if let Some(phone) = to.phone.as_deref().and_then(e164) {
        data.insert("sha256_phone_number".into(), json!(sha256(&phone)));
    }
    let mut address = Map::new();
    let name = to.name.trim().to_lowercase();
    let (first, last) = name.split_once(' ').unwrap_or((&name, ""));
    address.insert("sha256_first_name".into(), json!(sha256(first.trim())));
    if !last.trim().is_empty() {
        address.insert("sha256_last_name".into(), json!(sha256(last.trim())));
    }
    address.insert("street".into(), json!(to.line1));
    address.insert("city".into(), json!(to.city));
    address.insert("postal_code".into(), json!(to.postal_code));
    address.insert("country".into(), json!(to.country));
    data.insert("address".into(), Value::Object(address));
    Value::Object(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag() -> GoogleTagSettings {
        GoogleTagSettings {
            analytics_id: Some("G-ABC123DEF4".into()),
            ads_id: Some("AW-123456789".into()),
            purchase_label: Some("SaLe".into()),
            add_to_cart_label: None,
            begin_checkout_label: Some("ChEcK".into()),
            enhanced_conversions: true,
        }
    }

    fn mug() -> TagItem {
        TagItem {
            id: "MUG-BLUE".into(),
            name: "Blue </script> Mug".into(),
            variant: Some("Bleu".into()),
            unit_cents: 1250,
            quantity: 2,
        }
    }

    const ADS: Option<Consent> = Some(Consent {
        analytics: false,
        ads: true,
    });

    #[test]
    fn the_tag_is_held_back_until_the_mode_lets_it_load() {
        assert_eq!(google_tag_head(&tag(), ConsentMode::Basic, None), None);
        assert_eq!(
            google_tag_head(&tag(), ConsentMode::Basic, Some(Consent::NONE)),
            None
        );
        let head = google_tag_head(&tag(), ConsentMode::Basic, ADS).unwrap();
        assert_eq!(
            head.src,
            "https://www.googletagmanager.com/gtag/js?id=G-ABC123DEF4"
        );
        assert_eq!(
            head.script,
            "gtag('js',new Date());gtag('config','G-ABC123DEF4');gtag('config','AW-123456789',{allow_enhanced_conversions:true});"
        );
        // Advanced mode loads it at once: the signals hold it back.
        assert!(google_tag_head(&tag(), ConsentMode::Advanced, None).is_some());
        // A shop with no tag has none in any mode.
        assert_eq!(
            google_tag_head(
                &GoogleTagSettings::default(),
                ConsentMode::Advanced,
                Some(Consent::ALL)
            ),
            None
        );
        // Google Ads alone loads under its own id.
        let ads_only = GoogleTagSettings {
            analytics_id: None,
            enhanced_conversions: false,
            ..tag()
        };
        let head = google_tag_head(&ads_only, ConsentMode::Advanced, None).unwrap();
        assert!(head.src.ends_with("?id=AW-123456789"));
        assert_eq!(
            head.script,
            "gtag('js',new Date());gtag('config','AW-123456789');"
        );
    }

    #[test]
    fn an_event_goes_to_analytics_and_to_the_conversion_that_has_a_label() {
        // No label for the cart: Analytics alone hears of it.
        let added = add_to_cart_events(&tag(), "EUR", &[mug()]);
        assert_eq!(
            added,
            [json!(["event", "add_to_cart", {
                "currency": "EUR",
                "value": 25.0,
                "items": [{
                    "item_id": "MUG-BLUE",
                    "item_name": "Blue </script> Mug",
                    "item_variant": "Bleu",
                    "price": 12.5,
                    "quantity": 2,
                }],
                "send_to": "G-ABC123DEF4",
            }])]
        );
        // Nothing in the page's data can close its script.
        let text = tag_events_json(&added);
        assert!(text.contains(r"Blue \u003c/script\u003e Mug"), "{text}");
        assert!(!text.contains('<'));

        let begun = begin_checkout_events(&tag(), "EUR", 2250, &[mug()]);
        assert_eq!(begun.len(), 2);
        assert_eq!(begun[0][1], "begin_checkout");
        assert_eq!(
            begun[1],
            json!(["event", "conversion", {
                "send_to": "AW-123456789/ChEcK",
                "value": 22.5,
                "currency": "EUR",
            }])
        );
        // Google Ads alone, without a label for it: nothing to send.
        let ads_only = GoogleTagSettings {
            analytics_id: None,
            ..tag()
        };
        assert!(add_to_cart_events(&ads_only, "EUR", &[mug()]).is_empty());
    }

    #[test]
    fn an_e_mail_address_and_a_phone_are_normalized_before_hashing() {
        assert_eq!(
            normalized_email(" Ada.Love.Lace@GMail.com "),
            "adalovelace@gmail.com"
        );
        assert_eq!(
            normalized_email("Ada.Lovelace@Example.org"),
            "ada.lovelace@example.org"
        );
        assert_eq!(e164("+33 6 12 34 56 78").as_deref(), Some("+33612345678"));
        assert_eq!(e164("06 12 34 56 78"), None);
        assert_eq!(e164("+33"), None);
        let buyer = user_data(
            " Ada@Example.org ",
            &ShippingAddress {
                name: " Ada  King Lovelace ".into(),
                line1: "1 rue de la Paix".into(),
                postal_code: "75002".into(),
                city: "Paris".into(),
                country: "FR".into(),
                phone: Some("+33 6 12 34 56 78".into()),
                ..Default::default()
            },
        );
        assert_eq!(buyer["sha256_email_address"], sha256("ada@example.org"));
        assert_eq!(buyer["sha256_phone_number"], sha256("+33612345678"));
        assert_eq!(buyer["address"]["sha256_first_name"], sha256("ada"));
        assert_eq!(
            buyer["address"]["sha256_last_name"],
            sha256("king lovelace")
        );
        assert_eq!(buyer["address"]["postal_code"], "75002");
        assert_eq!(buyer["address"]["country"], "FR");
        // A known digest, so the hash is the one Google computes too.
        assert_eq!(
            sha256("test@example.com"),
            "973dfe463ec85785f5f95af5ba3906eedb2d931c24e69824a89ea65dba4e813b"
        );
    }
}
