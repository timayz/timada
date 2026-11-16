//! The shop's Google tag and Meta pixel on the pages: whether this visitor
//! gets them, and the events the pages send through them.
//!
//! Both are the merchant's, set up in the back office; the layout writes
//! the tag in every `<head>` unless the visitor's answer to the cookie
//! banner holds it back ([`CONSENT_MODE`]), and the pixel for a visitor who
//! allowed advertising. Three events go through both: an item added to the
//! cart (noted by the form's handler in a cookie, sent by the next page),
//! the checkout opened, and a sale, once per order, from the order's page
//! right after the payment. The pixel also hears of every page, and of the
//! product a page shows.

use serde_json::Value;
use timada_storefront::{
    Browser, CartView, GoogleTagSettings, OrderState, Product, TagItem, Variant,
    add_to_cart_events, begin_checkout_events, checkout_browser, google_tag,
    initiate_checkout_event, meta_pixel, pixel_allowed, pixel_event_id, pixel_purchase_event,
    purchase_events, view_content_event,
};
use topcoat::{Result, context::Cx};

use crate::app::CONSENT_MODE;
use crate::shopper::{consent, remember_added, take_purchase};

/// The shop's tag when this visitor's pages carry it: the shop set one up
/// and the answer to the cookie banner lets it load.
pub async fn tag_in_use(cx: &Cx) -> Result<Option<GoogleTagSettings>> {
    if !CONSENT_MODE.loads(consent(cx)) {
        return Ok(None);
    }
    google_tag(cx).await
}

/// The shop's pixel when this visitor's pages carry it: the shop set one
/// and the visitor allowed advertising.
pub async fn pixel_in_use(cx: &Cx) -> Result<Option<String>> {
    if !pixel_allowed(consent(cx)) {
        return Ok(None);
    }
    meta_pixel(cx).await
}

/// What a page tells of itself: the Google tag's events, and the pixel's.
#[derive(Debug, Clone, Default)]
pub struct Told {
    pub tag: Vec<Value>,
    pub pixel: Vec<Value>,
}

/// This request's browser, for the order's sale to be told to Meta from
/// the server as well: only in a shop with a pixel, for a shopper who
/// allowed advertising.
pub async fn browser(cx: &Cx) -> Result<Option<Browser>> {
    if pixel_in_use(cx).await?.is_none() {
        return Ok(None);
    }
    Ok(checkout_browser(cx, consent(cx)))
}

/// What went into the cart, kept for the next page to send.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Added {
    pub currency: String,
    pub items: Vec<TagItem>,
}

/// A product (one `variant` of it) as an event lists it, with the
/// currency of its price.
fn item_of(product: &Product, variant: Option<&Variant>, quantity: u32) -> (TagItem, String) {
    let (cents, currency) = match variant {
        Some(variant) => (variant.amount_cents, variant.currency.clone()),
        None => (
            product
                .amount_cents
                .or(product.min_amount_cents)
                .unwrap_or_default(),
            product.currency.clone().unwrap_or_default(),
        ),
    };
    let item = TagItem {
        id: variant
            .and_then(|variant| variant.sku.clone())
            .unwrap_or_else(|| product.id.clone()),
        name: product.name.clone(),
        variant: variant.map(|variant| {
            variant
                .selections
                .iter()
                .map(|selection| selection.value.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        }),
        unit_cents: cents.max(0) as u64,
        quantity,
    };
    (item, currency)
}

/// Notes that `quantity` of a product (one `variant` of it) went into the
/// cart, on a page that carries the tag or the pixel: the page the shopper
/// lands on next sends it.
pub async fn note_added(
    cx: &Cx,
    product: &Product,
    variant: Option<&Variant>,
    quantity: u32,
) -> Result<()> {
    if tag_in_use(cx).await?.is_none() && pixel_in_use(cx).await?.is_none() {
        return Ok(());
    }
    let (item, currency) = item_of(product, variant, quantity);
    remember_added(
        cx,
        Added {
            currency,
            items: vec![item],
        },
    )
}

/// What a product's page tells the pixel: the product shown, or the
/// variant the page opened on. Nothing on a page without the pixel.
pub async fn product_events(cx: &Cx, product: &Product, shown: Option<&Variant>) -> Result<Told> {
    let mut told = Told::default();
    if pixel_in_use(cx).await?.is_some() {
        let (item, currency) = item_of(product, shown, 1);
        told.pixel
            .push(view_content_event(&currency, &item, &pixel_event_id()));
    }
    Ok(told)
}

/// The event of what was noted as added.
pub fn added_events(tag: &GoogleTagSettings, added: &Added) -> Vec<Value> {
    add_to_cart_events(tag, &added.currency, &added.items)
}

/// The events of the checkout opening on this cart; nothing on a page
/// without the tag or the pixel.
pub async fn checkout_events(cx: &Cx, cart: &CartView) -> Result<Told> {
    let tag = tag_in_use(cx).await?;
    let pixel = pixel_in_use(cx).await?;
    let mut told = Told::default();
    if tag.is_none() && pixel.is_none() {
        return Ok(told);
    }
    let items: Vec<TagItem> = cart
        .lines
        .iter()
        .map(|line| TagItem {
            id: line.sku.clone().unwrap_or_else(|| line.product_id.clone()),
            name: line.name.clone(),
            variant: line.variant_label.clone(),
            unit_cents: line.unit_amount_cents.unwrap_or_default(),
            quantity: line.quantity,
        })
        .collect();
    let currency = cart.currency.as_deref().unwrap_or_default();
    if let Some(tag) = &tag {
        told.tag = begin_checkout_events(tag, currency, cart.total_cents, &items);
    }
    if pixel.is_some() {
        told.pixel.push(initiate_checkout_event(
            currency,
            cart.total_cents,
            &items,
            &pixel_event_id(),
        ));
    }
    Ok(told)
}

/// The sale, when this is the first look at the order's page since its
/// payment: once per order, whether or not the page carries the tag or the
/// pixel (a visitor who refused is not counted later either).
pub async fn sale_events(cx: &Cx, order: &OrderState) -> Result<Told> {
    let mut told = Told::default();
    if !take_purchase(cx, &order.id) {
        return Ok(told);
    }
    if let Some(tag) = tag_in_use(cx).await? {
        told.tag = purchase_events(&tag, order, consent(cx));
    }
    if pixel_in_use(cx).await?.is_some() {
        told.pixel.push(pixel_purchase_event(order));
    }
    Ok(told)
}
