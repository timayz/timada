//! The shop's Meta pixel: its id, as the merchant set it in the back office
//! (`timada-pages`' `MetaPixel`), copied into `storefront_meta_pixel` by the
//! catalog subscription, and the scripts a page writes from it.
//!
//! Meta has no mode that runs without cookies: [`meta_pixel_head`] gives the
//! pixel only for a visitor who allowed advertising, and nothing of Meta's
//! is written for anyone else. [`page_view_event`], [`view_content_event`],
//! [`pixel_add_to_cart_event`], [`initiate_checkout_event`] and
//! [`pixel_purchase_event`] give the events, as the arguments of one
//! `fbq(…)` call each, with the event's id last.
//!
//! A page writes its events as data ([`tag_events_json`](crate::tag_events_json),
//! in a `<script type="application/json" data-pixel-events>`) and
//! [`PIXEL_EVENTS_SCRIPT`] after it, which sends them; a script that fetched
//! another page hands it to `timadaPixel(…)`, as the Google tag's does.
//!
//! A sale's event id is the order's id: the dashboard sends the same sale
//! to Meta's Conversions API under it, when the shop gave a token and the
//! checkout said the shopper allowed advertising ([`checkout_browser`]), and
//! Meta counts the two once.

use evento::{Executor, metadata::RawEvent, subscription::Context};
use rand::RngCore;
use serde_json::{Map, Value, json};
use timada_core::db::Db;
use timada_orders::OrderState;

use crate::source::Browser;
use timada_pages::aggregator::MetaPixel;
use timada_pages::meta_pixel_projection;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};
use topcoat_router::request::{headers, original_uri};

use crate::consent::Consent;
use crate::context::store;
use crate::google_tag::{TagItem, order_items};
use crate::sitemap::absolute_url;

/// The pixel's id as the copy holds it; `None` for a shop that set none.
pub async fn meta_pixel_setting(db: &Db) -> anyhow::Result<Option<String>> {
    let row: Option<(Option<String>,)> = db
        .fetch_optional(
            "SELECT pixel_id FROM storefront_meta_pixel WHERE id = 1",
            (),
        )
        .await?;
    Ok(row.and_then(|(pixel_id,)| pixel_id))
}

/// Whether a sale is worth its profit to Meta, as the copy holds the
/// shop's choice: its total otherwise.
pub async fn meta_profit_as_value_setting(db: &Db) -> anyhow::Result<bool> {
    let row: Option<(i64,)> = db
        .fetch_optional(
            "SELECT profit_as_value FROM storefront_meta_pixel WHERE id = 1",
            (),
        )
        .await?;
    Ok(row.is_some_and(|(profit,)| profit != 0))
}

/// Whether the shop's pixel tells a sale's profit as its value; see
/// [`pixel_purchase_event`].
pub async fn meta_profit_as_value(cx: &Cx) -> Result<bool> {
    meta_profit_as_value_setting(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// The shop's Meta pixel id, when it set one up; see the module docs.
pub async fn meta_pixel(cx: &Cx) -> Result<Option<String>> {
    meta_pixel_setting(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// Rewrites the one row from the stream.
#[evento::subscription_all]
pub(crate) async fn on_meta_pixel_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<MetaPixel>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let pixel = meta_pixel_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?;
    let pixel_id = pixel.as_ref().and_then(|state| state.pixel_id.as_deref());
    let profit_as_value = pixel.as_ref().is_some_and(|state| state.profit_as_value);
    db.execute(
        "INSERT INTO storefront_meta_pixel (id, pixel_id, profit_as_value) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET pixel_id = excluded.pixel_id,
            profit_as_value = excluded.profit_as_value",
        (pixel_id, i64::from(profit_as_value)),
    )
    .await?;
    Ok(())
}

/// Whether Meta's pixel may be written for a visitor who answered the
/// cookie banner with `answer`: only once they allowed advertising.
pub fn pixel_allowed(answer: Option<Consent>) -> bool {
    answer.is_some_and(|consent| consent.ads)
}

/// The inline script that loads Meta's pixel and starts it under
/// `pixel_id`, for a page's `<head>`; nothing for a visitor who did not
/// allow advertising. The id is digits only (the back office checks it).
pub fn meta_pixel_head(pixel_id: &str, answer: Option<Consent>) -> Option<String> {
    if !pixel_allowed(answer) || !pixel_id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "!function(f,b,e,v,n,t,s){{if(f.fbq)return;n=f.fbq=function(){{n.callMethod?n.callMethod.apply(n,arguments):n.queue.push(arguments)}};if(!f._fbq)f._fbq=n;n.push=n;n.loaded=!0;n.version='2.0';n.queue=[];t=b.createElement(e);t.async=!0;t.src=v;t.setAttribute('data-meta-pixel','');s=b.getElementsByTagName(e)[0];s.parentNode.insertBefore(t,s)}}(window,document,'script','https://connect.facebook.net/en_US/fbevents.js');fbq('init','{pixel_id}');"
    ))
}

/// Sends the events a document carries in its `data-pixel-events` scripts,
/// once each. Written after the events of the page itself, on a page that
/// has the pixel. It leaves `timadaPixel(page, true)` for a script that
/// fetched another page to show a part of it: that sends only what the
/// fetched page marked `data-pixel-events="added"` (an item just added to
/// the cart), not the page view the shopper never had.
pub const PIXEL_EVENTS_SCRIPT: &str = "window.timadaPixel=function(d,a){d.querySelectorAll(a?'script[data-pixel-events=added]':'script[data-pixel-events]').forEach(function(s){if(s.dataset.sent)return;s.dataset.sent='1';try{JSON.parse(s.textContent).forEach(function(c){fbq.apply(null,c)})}catch(e){}})};timadaPixel(document);";

/// An id for one event, random: what Meta tells a browser's event and a
/// server's apart by. A sale takes the order's id instead.
pub fn pixel_event_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An amount in minor units as the number Meta reads (`19.9`).
fn amount(cents: u64) -> Value {
    json!(cents as f64 / 100.0)
}

/// The arguments of `fbq('track', name, params, {eventID})`.
fn track(name: &str, params: Map<String, Value>, event_id: &str) -> Value {
    json!(["track", name, params, { "eventID": event_id }])
}

/// What Meta's standard events say of the things they are about: their
/// ids (the shopping feed's), each with its quantity and unit price.
fn contents(params: &mut Map<String, Value>, items: &[TagItem]) {
    params.insert("content_type".into(), json!("product"));
    params.insert(
        "content_ids".into(),
        Value::Array(items.iter().map(|item| json!(item.id)).collect()),
    );
    params.insert(
        "contents".into(),
        Value::Array(
            items
                .iter()
                .map(|item| {
                    json!({
                        "id": item.id,
                        "quantity": item.quantity,
                        "item_price": amount(item.unit_cents),
                    })
                })
                .collect(),
        ),
    );
}

fn value_of(items: &[TagItem]) -> u64 {
    items
        .iter()
        .map(|item| item.unit_cents * u64::from(item.quantity))
        .sum()
}

fn count_of(items: &[TagItem]) -> u64 {
    items.iter().map(|item| u64::from(item.quantity)).sum()
}

/// `PageView`: every page tells it.
pub fn page_view_event(event_id: &str) -> Value {
    track("PageView", Map::new(), event_id)
}

/// `ViewContent`: a product's page, about the thing it shows.
pub fn view_content_event(currency: &str, shown: &TagItem, event_id: &str) -> Value {
    let mut params = Map::new();
    contents(&mut params, std::slice::from_ref(shown));
    params.insert("content_name".into(), json!(shown.name));
    params.insert("value".into(), amount(shown.unit_cents));
    params.insert("currency".into(), json!(currency));
    track("ViewContent", params, event_id)
}

/// `AddToCart`: what a page sends once something went into the cart.
pub fn pixel_add_to_cart_event(currency: &str, added: &[TagItem], event_id: &str) -> Value {
    let mut params = Map::new();
    contents(&mut params, added);
    params.insert("value".into(), amount(value_of(added)));
    params.insert("currency".into(), json!(currency));
    track("AddToCart", params, event_id)
}

/// `InitiateCheckout`: the checkout's page, valued at what the cart's
/// goods cost.
pub fn initiate_checkout_event(
    currency: &str,
    value_cents: u64,
    cart: &[TagItem],
    event_id: &str,
) -> Value {
    let mut params = Map::new();
    contents(&mut params, cart);
    params.insert("num_items".into(), json!(count_of(cart)));
    params.insert("value".into(), amount(value_cents));
    params.insert("currency".into(), json!(currency));
    track("InitiateCheckout", params, event_id)
}

/// `Purchase`: what the order's confirmation page sends, once per order
/// (the host sees to that), with the order's total as its value, or its
/// profit for a shop that bids on profit (`profit`,
/// [`OrderState::conversion_value_cents`]), and the order's id as the
/// event's, under which the dashboard tells Meta the same sale from the
/// server, worth the same.
pub fn pixel_purchase_event(order: &OrderState, profit: bool) -> Value {
    let items = order_items(order);
    let mut params = Map::new();
    contents(&mut params, &items);
    params.insert("num_items".into(), json!(count_of(&items)));
    params.insert("value".into(), amount(order.conversion_value_cents(profit)));
    params.insert("currency".into(), json!(order.currency));
    params.insert("order_id".into(), json!(order.reference));
    track("Purchase", params, &order.id)
}

/// The value of the cookie `name` as the browser sent it.
fn raw_cookie(cx: &Cx, name: &str) -> Option<String> {
    headers(cx)
        .get_all("cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (found, value) = pair.trim().split_once('=')?;
            (found == name).then(|| value.to_owned())
        })
}

/// What the checkout hands to the order of this request's browser, for a
/// shopper who allowed advertising: its `User-Agent`, the ids Meta's pixel
/// keeps in its `_fbp` and `_fbc` cookies, and the page's address. With it
/// the dashboard may tell Meta the sale from the server; without, it tells
/// nobody. `None` for a visitor who did not allow advertising.
pub fn checkout_browser(cx: &Cx, answer: Option<Consent>) -> Option<Browser> {
    if !pixel_allowed(answer) {
        return None;
    }
    let user_agent = headers(cx)
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    Some(Browser {
        user_agent,
        fbp: raw_cookie(cx, "_fbp"),
        fbc: raw_cookie(cx, "_fbc"),
        page: Some(absolute_url(cx, original_uri(cx).path())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADS: Option<Consent> = Some(Consent {
        analytics: false,
        ads: true,
    });

    fn mug() -> TagItem {
        TagItem {
            id: "MUG-BLUE".into(),
            name: "Blue Mug".into(),
            variant: Some("Bleu".into()),
            unit_cents: 1250,
            quantity: 2,
        }
    }

    #[test]
    fn the_pixel_waits_for_advertising_to_be_allowed() {
        assert_eq!(meta_pixel_head("1234567890", None), None);
        assert_eq!(meta_pixel_head("1234567890", Some(Consent::NONE)), None);
        // Audience measurement alone is not advertising.
        assert_eq!(
            meta_pixel_head(
                "1234567890",
                Some(Consent {
                    analytics: true,
                    ads: false
                })
            ),
            None
        );
        let head = meta_pixel_head("1234567890", ADS).unwrap();
        assert!(head.contains("https://connect.facebook.net/en_US/fbevents.js"));
        assert!(head.ends_with("fbq('init','1234567890');"));
        // Nothing but digits is ever written into the script.
        assert_eq!(meta_pixel_head("1');alert(1)//", ADS), None);
    }

    #[test]
    fn an_event_carries_its_things_and_its_id() {
        assert_eq!(
            page_view_event("abc"),
            json!(["track", "PageView", {}, { "eventID": "abc" }])
        );
        assert_eq!(
            pixel_add_to_cart_event("EUR", &[mug()], "e1"),
            json!(["track", "AddToCart", {
                "content_type": "product",
                "content_ids": ["MUG-BLUE"],
                "contents": [{ "id": "MUG-BLUE", "quantity": 2, "item_price": 12.5 }],
                "value": 25.0,
                "currency": "EUR",
            }, { "eventID": "e1" }])
        );
        let seen = view_content_event("EUR", &mug(), "e2");
        assert_eq!(seen[1], "ViewContent");
        assert_eq!(seen[2]["value"], 12.5);
        assert_eq!(seen[2]["content_name"], "Blue Mug");
        let begun = initiate_checkout_event("EUR", 2990, &[mug()], "e3");
        assert_eq!(begun[2]["num_items"], 2);
        assert_eq!(begun[2]["value"], 29.9);
        assert_eq!(begun[3]["eventID"], "e3");
    }

    #[test]
    fn a_sale_is_told_under_the_orders_id() {
        let order = OrderState {
            id: "order-1".into(),
            reference: "A-1001".into(),
            total_cents: 2990,
            currency: "EUR".into(),
            ..Default::default()
        };
        let sale = pixel_purchase_event(&order, false);
        assert_eq!(sale[1], "Purchase");
        assert_eq!(sale[2]["value"], 29.9);
        assert_eq!(sale[2]["currency"], "EUR");
        assert_eq!(sale[2]["order_id"], "A-1001");
        assert_eq!(sale[3]["eventID"], "order-1");

        // A shop that bids on profit: what the store API told the sale left,
        // or the total when it told none.
        assert_eq!(pixel_purchase_event(&order, true)[2]["value"], 29.9);
        let order = OrderState {
            conversion_profit_cents: Some(1042),
            ..order
        };
        assert_eq!(pixel_purchase_event(&order, true)[2]["value"], 10.42);
        assert_eq!(pixel_purchase_event(&order, false)[2]["value"], 29.9);
    }

    #[test]
    fn event_ids_differ() {
        let id = pixel_event_id();
        assert_eq!(id.len(), 32);
        assert_ne!(id, pixel_event_id());
    }
}
