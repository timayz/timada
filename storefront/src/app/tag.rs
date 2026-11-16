//! The shop's Google tag on the pages: whether this visitor gets it, and
//! the events the pages send through it.
//!
//! The tag is the merchant's, set up in the back office; the layout writes
//! it in every `<head>` unless the visitor's answer to the cookie banner
//! holds it back ([`CONSENT_MODE`]). Three events go through it: an item
//! added to the cart (noted by the form's handler in a cookie, sent by the
//! next page), the checkout opened, and a sale, once per order, from the
//! order's page right after the payment.

use serde_json::Value;
use timada_storefront::{
    CartView, GoogleTagSettings, OrderState, Product, TagItem, Variant, add_to_cart_events,
    begin_checkout_events, google_tag, purchase_events,
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

/// What went into the cart, kept for the next page to send.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Added {
    pub currency: String,
    pub items: Vec<TagItem>,
}

/// Notes that `quantity` of a product (one `variant` of it) went into the
/// cart, on a page that carries the tag: the page the shopper lands on next
/// sends it.
pub async fn note_added(
    cx: &Cx,
    product: &Product,
    variant: Option<&Variant>,
    quantity: u32,
) -> Result<()> {
    if tag_in_use(cx).await?.is_none() {
        return Ok(());
    }
    let (cents, currency) = match variant {
        Some(variant) => (variant.amount_cents, variant.currency.clone()),
        None => (
            product.amount_cents.unwrap_or_default(),
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
    remember_added(
        cx,
        Added {
            currency,
            items: vec![item],
        },
    )
}

/// The event of what was noted as added.
pub fn added_events(tag: &GoogleTagSettings, added: &Added) -> Vec<Value> {
    add_to_cart_events(tag, &added.currency, &added.items)
}

/// The event of the checkout opening on this cart; nothing on a page
/// without the tag.
pub async fn checkout_events(cx: &Cx, cart: &CartView) -> Result<Vec<Value>> {
    let Some(tag) = tag_in_use(cx).await? else {
        return Ok(Vec::new());
    };
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
    Ok(begin_checkout_events(
        &tag,
        cart.currency.as_deref().unwrap_or_default(),
        cart.total_cents,
        &items,
    ))
}

/// The sale, when this is the first look at the order's page since its
/// payment: once per order, whether or not the page carries the tag (a
/// visitor who refused is not counted later either).
pub async fn sale_events(cx: &Cx, order: &OrderState) -> Result<Vec<Value>> {
    if !take_purchase(cx, &order.id) {
        return Ok(Vec::new());
    }
    let Some(tag) = tag_in_use(cx).await? else {
        return Ok(Vec::new());
    };
    Ok(purchase_events(&tag, order, consent(cx)))
}
