//! The checkout: the shopper's details, the address, the shipping method
//! and the payment beside the cart. `?error=` carries a refusal; `?address=`
//! and `?billing=` pick saved addresses of the signed-in shopper.
//!
//! The page after the payment (`checkout_complete`) waits for the order
//! the payment became, then sends the shopper to it.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::error::redirect;

use crate::actions::query;
use crate::cart::{
    CartView, OfferView, PaymentSetup, ShippingOption, VatEstimate, accepts_business_buyers,
    order_of_cart, payment_setup,
};
use crate::context::menu;
use crate::query::CategoryNode;
use crate::reviews::{Review, recent_reviews};
use crate::session::{
    CheckoutDraft, accounts_enabled, checkout_draft, current_cart, forget_cart,
    payment_in_progress, remember_order, save_checkout_draft, signed_in,
};
use crate::shoppers::Shopper;
use crate::storefront::{actions, messages, money_format, paths, storefront};

use super::{Flash, LineModel, flash, line_models};

#[derive(serde::Deserialize)]
struct CheckoutQuery {
    error: Option<String>,
    address: Option<String>,
    billing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutOptions {
    /// How many recent reviews the page shows to reassure.
    pub reviews: u32,
}

impl Default for CheckoutOptions {
    fn default() -> Self {
        Self { reviews: 3 }
    }
}

/// A country the checkout lists, with what the VAT line says for it and
/// each shipping method (`""` for none), as JSON for the page's script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountryOption {
    pub code: String,
    pub name: String,
    pub vat_json: String,
}

/// A shipping method as the form offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingChoice {
    pub option: ShippingOption,
    /// Whether it ships to the country picked.
    pub ships_here: bool,
    pub picked: bool,
    /// Its price written the shop's way, or what the shop calls free.
    pub price: String,
}

/// An offer the shopper may add, with its prices written the shop's way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferModel {
    pub offer: OfferView,
    pub price: String,
    pub offer_price: String,
    pub percent_off: u64,
    /// The id of its form, which sits outside the checkout's (forms do not
    /// nest); a button anywhere submits it with `form=`.
    pub form_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutModel {
    pub nav: Vec<CategoryNode>,
    pub cart: CartView,
    pub lines: Vec<LineModel>,
    pub currency: String,
    pub setup: Option<PaymentSetup>,
    pub publishable_key: String,
    pub reviews: Vec<Review>,
    /// Whether the shop sells to businesses: the VAT number field shows.
    pub business: bool,
    pub accounts_enabled: bool,
    /// What the form starts filled with.
    pub draft: CheckoutDraft,
    pub shopper: Option<Shopper>,
    /// The saved addresses shipped to and billed to, by id.
    pub shipping_to: Option<String>,
    pub billing_to: Option<String>,
    pub sign_in_url: String,
    /// The delivery country picked (ISO 3166-1 alpha-2), and the billing one.
    pub country: String,
    pub billing_country: String,
    pub bills_elsewhere: bool,
    pub countries: Vec<CountryOption>,
    /// The shipping method priced in: the one picked last when it ships
    /// here, else the first that does.
    pub shipping: Option<ShippingOption>,
    pub shipping_choices: Vec<ShippingChoice>,
    /// Whether the form asks for a method: the shop has some.
    pub asks_shipping: bool,
    /// What the shipping line says: a price, free, included, or none ships here.
    pub shipping_text: String,
    /// `incl. 4.17 EUR VAT FR 20%`, when VAT applies and is shown.
    pub vat_text: Option<String>,
    pub subtotal: String,
    pub discount_amount: Option<String>,
    pub discount_refused: Option<Flash>,
    /// What the goods cost, before shipping.
    pub goods_cents: u64,
    pub total_cents: u64,
    pub total: String,
    /// The shipping methods as a wallet offers them, JSON.
    pub rates: String,
    /// How the script writes money, JSON.
    pub money: String,
    pub offers: Vec<OfferModel>,
    pub error: Option<Flash>,
    pub cart_url: String,
    pub checkout_url: String,
    pub place_url: String,
    pub start_url: String,
    pub return_url: String,
    pub seller_country: String,
    pub accent_color: String,
    pub standard_shipping: String,
    pub something_went_wrong: String,
}

impl CheckoutModel {
    /// The checkout shipping to that saved address (billing as now).
    pub fn ship_here_url(&self, cx: &Cx, address_id: &str) -> String {
        paths(cx).checkout_saved_url(Some(address_id), self.billing_to.as_deref())
    }

    /// The checkout billing to that saved address (shipping as now).
    pub fn bill_here_url(&self, cx: &Cx, address_id: &str) -> String {
        paths(cx).checkout_saved_url(self.shipping_to.as_deref(), Some(address_id))
    }
}

/// `incl. 9.97 EUR VAT FR 20%`.
fn vat_text(cx: &Cx, estimate: &VatEstimate) -> String {
    format!(
        "{} {} {}",
        messages(cx).vat_prefix,
        money_format(cx).format(estimate.vat_cents, &estimate.currency),
        estimate.label()
    )
}

/// The shop's shipping options, as a wallet's rates (with the countries
/// each ships to): `[]` when shipping is included.
pub fn express_rates(cart: &CartView) -> String {
    let rates: Vec<_> = cart
        .shipping_options
        .iter()
        .map(|option| {
            serde_json::json!({
                "id": option.id,
                "displayName": option.name,
                "amount": option.amount_cents,
                "countries": option.countries,
            })
        })
        .collect();
    serde_json::Value::Array(rates).to_string()
}

/// Whether the VAT line shows: not for a business buyer abroad, whose
/// invoice carries no VAT.
fn shows_vat(cx: &Cx, vat_number: &str, country: &str) -> bool {
    vat_number.trim().is_empty()
        || storefront(cx)
            .seller_country
            .as_deref()
            .is_some_and(|seller| seller == country)
}

/// Redirects to the cart when nothing can be ordered (or something off sale
/// must be removed first).
pub async fn checkout_page(cx: &Cx, options: &CheckoutOptions) -> Result<CheckoutModel> {
    let query: CheckoutQuery = query(cx)?;
    let paths = paths(cx);
    let Some(cart) = current_cart(cx).await?.filter(|cart| cart.can_check_out()) else {
        return Err(redirect(paths.cart_url(None)).into());
    };
    let nav = menu(cx).await?;
    let setup = payment_setup(cx).await?;
    let reviews = recent_reviews(cx, options.reviews.max(1)).await?;
    let business = accepts_business_buyers(cx).await?;
    let mut draft = checkout_draft(cx);
    if !business {
        draft.vat_number.clear();
    }
    let accounts = accounts_enabled(cx);
    let shopper = if accounts {
        signed_in(cx).await.cloned()
    } else {
        None
    };
    // Signed in: the account's address, and the saved address chosen (or the
    // latest one, when nothing was typed yet); the one chosen to bill to.
    let mut shipping_to = None;
    let mut billing_to = None;
    if let Some(shopper) = &shopper {
        draft.email = shopper.email.clone();
        let pick = match query.address.as_deref() {
            Some(id) => shopper.address(id),
            None if draft.line1.trim().is_empty() => shopper.addresses.first(),
            None => None,
        };
        if let Some(saved) = pick {
            shipping_to = Some(saved.id.clone());
            draft = draft.with_address(saved);
        }
        if let Some(saved) = query.billing.as_deref().and_then(|id| shopper.address(id)) {
            billing_to = Some(saved.id.clone());
            draft = draft.with_billing_address(saved);
        }
    }
    let messages = messages(cx);
    let money = money_format(cx);
    let currency = cart.currency.clone().unwrap_or_default();
    let country = if draft.country.is_empty() {
        messages.default_country.to_string()
    } else {
        draft.country.to_ascii_uppercase()
    };
    let billing_country = if draft.billing_country.is_empty() {
        country.clone()
    } else {
        draft.billing_country.to_ascii_uppercase()
    };
    let shipping = cart
        .shipping_options_to(&country)
        .find(|option| option.id == draft.shipping_method)
        .or_else(|| cart.shipping_options_to(&country).next())
        .cloned();
    let price_of = |option: &ShippingOption| match option.amount_cents {
        0 => messages.shipping_free.to_string(),
        cents => money.format(cents, &option.currency),
    };
    let shipping_text = match &shipping {
        Some(option) => price_of(option),
        None if cart.shipping_required => messages.shipping_unavailable.to_string(),
        None => messages.shipping_included.to_string(),
    };
    let shipping_choices = cart
        .shipping_options
        .iter()
        .map(|option| ShippingChoice {
            ships_here: option.ships_to(&country),
            picked: shipping
                .as_ref()
                .is_some_and(|picked| picked.id == option.id),
            price: price_of(option),
            option: option.clone(),
        })
        .collect();
    // What the VAT line says for each country and shipping method (`""`
    // for none), for the page's script to switch.
    let countries = messages
        .countries
        .iter()
        .map(|(code, name)| {
            let mut texts = serde_json::Map::new();
            for option in std::iter::once(None).chain(cart.shipping_options_to(code).map(Some)) {
                if let Some(estimate) = cart.vat_estimate_with(code, option) {
                    texts.insert(
                        option.map(|option| option.id.clone()).unwrap_or_default(),
                        vat_text(cx, &estimate).into(),
                    );
                }
            }
            CountryOption {
                code: (*code).to_owned(),
                name: name.to_string(),
                vat_json: serde_json::Value::Object(texts).to_string(),
            }
        })
        .collect();
    let vat = cart
        .vat_estimate_with(&country, shipping.as_ref())
        .as_ref()
        .map(|estimate| vat_text(cx, estimate))
        .filter(|_| shows_vat(cx, &draft.vat_number, &country));
    let total_cents = cart.total_with(shipping.as_ref());
    let offers = cart
        .offers
        .iter()
        .filter(|offer| offer.slug.is_some())
        .map(|offer| OfferModel {
            price: money.format(offer.unit_amount_cents, &offer.currency),
            offer_price: money.format(offer.offer_amount_cents, &offer.currency),
            percent_off: offer.percent_off(),
            form_id: offer_form_id(offer),
            offer: offer.clone(),
        })
        .collect();
    let lines = line_models(cx, &cart.lines, &currency);
    Ok(CheckoutModel {
        nav,
        lines,
        publishable_key: setup
            .as_ref()
            .map(|setup| setup.publishable_key.clone())
            .unwrap_or_default(),
        setup,
        reviews,
        business,
        accounts_enabled: accounts,
        shopper,
        shipping_to,
        billing_to,
        sign_in_url: paths.sign_in_url(Some(&paths.checkout_url(None)), None),
        bills_elsewhere: draft.bills_elsewhere(),
        country,
        billing_country,
        countries,
        shipping_choices,
        asks_shipping: !cart.shipping_options.is_empty() || cart.shipping_required,
        shipping_text,
        vat_text: vat,
        subtotal: money.format(cart.subtotal_cents, &currency),
        discount_amount: cart
            .discount
            .as_ref()
            .map(|discount| money.signed(-(discount.amount_cents as i64), &currency)),
        discount_refused: flash(cx, cart.discount_refused),
        goods_cents: cart.total_cents,
        total_cents,
        total: money.format(total_cents, &currency),
        rates: express_rates(&cart),
        money: money.descriptor(&currency),
        offers,
        error: flash(cx, query.error.as_deref()),
        cart_url: paths.cart_url(None),
        checkout_url: paths.checkout_url(None),
        place_url: actions(cx).checkout_place(),
        start_url: actions(cx).checkout_start(),
        return_url: paths.checkout_complete_url(),
        seller_country: storefront(cx).seller_country.clone().unwrap_or_default(),
        accent_color: storefront(cx).accent_color.clone().unwrap_or_default(),
        standard_shipping: messages.standard_shipping.to_string(),
        something_went_wrong: messages.something_went_wrong.to_string(),
        shipping,
        draft,
        currency,
        cart,
    })
}

/// The id of an offer's form: `offer-<offer id>`.
pub fn offer_form_id(offer: &OfferView) -> String {
    format!("offer-{}", offer.offer_id)
}

#[derive(serde::Deserialize)]
struct CompleteQuery {
    /// What the provider says about the confirmation: `succeeded`,
    /// `processing` or `failed`.
    redirect_status: Option<String>,
}

/// The page after the payment, while the order is not there yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteModel {
    pub nav: Vec<CategoryNode>,
    /// This page, to look again.
    pub retry_url: String,
}

/// Where the provider sends the browser back. The order exists once the
/// provider told the dashboard the payment went through, which may take a
/// moment longer than the browser's way back: redirects to the order once
/// it is there (forgetting the cart), to the checkout on a failed or expired
/// payment, and otherwise returns what the waiting page shows.
pub async fn checkout_complete(cx: &Cx) -> Result<CompleteModel> {
    let query: CompleteQuery = query(cx)?;
    let paths = paths(cx);
    let Some(progress) = payment_in_progress(cx) else {
        return Err(redirect(paths.checkout_url(Some("payment_expired"))).into());
    };
    if query.redirect_status.as_deref() == Some("failed") {
        return Err(redirect(paths.checkout_url(Some("payment_failed"))).into());
    }
    if let Some(order) = order_of_cart(cx, &progress.cart_id).await? {
        remember_order(cx, &order.id)?;
        forget_cart(cx);
        // The payment stays remembered: the page may be loaded twice at once
        // (its own reload racing the provider's return), and the second
        // load must find the order too. The next checkout replaces it.
        // The address stays for the next order; the note was for this one.
        let mut draft = checkout_draft(cx);
        draft.note.clear();
        save_checkout_draft(cx, draft)?;
        return Err(redirect(paths.order_url(&order.id)).into());
    }
    Ok(CompleteModel {
        nav: menu(cx).await?,
        retry_url: paths.checkout_complete_url(),
    })
}
