//! `/checkout`: one page from the cart to the payment. The shopper's
//! details, the address and Stripe's Payment Element sit side by side with
//! the cart, which can still be changed there. Stripe's form shows before
//! any payment is open (its "deferred" mode): the button asks to pay
//! (`/checkout/start`), which opens the payment for the cart as it stands, then
//! confirms it in the browser, which comes back to `/checkout/complete`. The
//! total the page showed travels with the request, so a price the shopper
//! did not see is never charged. The merchant's checkout offers show as
//! cards above the payment, each with an Add button that puts its item in
//! the cart and comes back here.

pub mod complete;
pub mod countries;
pub mod start;

use timada_storefront::{
    CartView, Checkout, OfferView, OrdersError, VatEstimate, accepts_business_buyers, format_money,
    menu, payment_setup, recent_reviews, request_language, request_time_zone, save_address,
    start_payment,
};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{RouterErrorExt, SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{Unescaped, View, view},
};

use crate::app::account::sign_in::sign_in_url;
use crate::app::cart::{cart_url, current_cart, discount, summary_line};
use crate::app::{document, say};
use crate::shopper::{
    CheckoutDraft, PaymentInProgress, accounts_enabled, cart_id, checkout_draft,
    save_checkout_draft, set_payment_in_progress, shopper_token, signed_in,
};
use countries::{COUNTRIES, DEFAULT_COUNTRY};

/// How many reviews the page shows beside the cart.
const REVIEWS_SHOWN: u32 = 3;

/// Mounts Stripe's Payment and Express Checkout elements before any payment
/// is open, keeps the VAT line in step with the country, and pays on submit:
/// ask the store to open the payment, then confirm it with Stripe. Static and
/// trusted: everything it needs comes from the form's `data-` attributes.
const CHECKOUT_SCRIPT: &str = r##"
const form = document.querySelector("[data-checkout-form]");
const button = form.querySelector("button.place");
const error = document.querySelector("[data-payment-error]");
const vat = document.querySelector("[data-checkout-vat]");
const country = form.elements.country;
// Absent when the shop does not sell to businesses.
const vatNumber = form.elements.vat_number;

// The shipping method picked, among those that ship to the country.
const methods = [...form.querySelectorAll("input[name=shipping_method]")];
const noShipping = form.querySelector("[data-no-shipping]");
const shippingLine = document.querySelector("[data-checkout-shipping]");
const totalLine = document.querySelector("[data-checkout-total]");
const placeTotal = form.querySelector("[data-place-total]");
const goods = Number(form.dataset.goods);
const money = (cents) => `${(cents / 100).toFixed(2)} ${form.dataset.currency.toUpperCase()}`;
const picked = () => methods.find((method) => method.checked && !method.disabled);

function showVat() {
  if (!vat) return;
  const texts = JSON.parse(country.selectedOptions[0]?.dataset.vat || "{}");
  const text = texts[picked()?.value || ""];
  const reverseCharge = !!vatNumber && vatNumber.value.trim() !== "" && country.value !== "FR";
  vat.hidden = !text || reverseCharge;
  vat.textContent = text ? `(${text})` : "";
}

// Shows the methods that ship to the country (keeping the one picked when
// it still does) and prices the order with the one picked.
function showShipping() {
  if (methods.length) {
    for (const method of methods) {
      const countries = method.dataset.countries;
      const ships = !countries || countries.split(" ").includes(country.value);
      method.disabled = !ships;
      method.closest("label").hidden = !ships;
    }
    if (!picked()) {
      const first = methods.find((method) => !method.disabled);
      if (first) first.checked = true;
    }
  }
  const method = picked();
  if (noShipping) noShipping.hidden = !methods.length || !!method;
  const total = goods + (method ? Number(method.dataset.amount) : 0);
  if (shippingLine && methods.length) shippingLine.textContent = method ? method.dataset.price : "—";
  totalLine.textContent = money(total);
  if (placeTotal) placeTotal.textContent = money(total);
  form.elements.expected_total_cents.value = total;
  form.dataset.amount = total;
  if (elements && total > 0) elements.update({ amount: total });
  showVat();
}
country.addEventListener("change", showShipping);
methods.forEach((method) => method.addEventListener("change", showShipping));
vatNumber?.addEventListener("input", showVat);

// The shop's shipping methods as Express Checkout offers them; none when
// shipping is included.
const rates = JSON.parse(form.dataset.shipping || "[]");
const allRates = rates.map(({ id, displayName, amount }) => ({ id, displayName, amount }));
const ratesTo = (code) => rates
  .filter((rate) => !rate.countries.length || rate.countries.includes(code))
  .map(({ id, displayName, amount }) => ({ id, displayName, amount }));

// The billing fields count only while the box is ticked: a disabled
// fieldset is neither checked nor sent.
const billing = form.querySelector("[data-billing]");
const billingDifferent = form.elements.billing_different;
function showBilling() {
  billing.hidden = !billingDifferent.checked;
  billing.disabled = !billingDifferent.checked;
}
billingDifferent.addEventListener("change", showBilling);

// The wallet's billing address, when it is not the delivery one.
function walletBilling(billing, shipping) {
  const at = billing.address, to = shipping.address || {};
  if (!at?.line1) return null;
  const same = (a, b) => (a || "").trim().toLowerCase() === (b || "").trim().toLowerCase();
  return ["line1", "line2", "postal_code", "city", "country"]
    .every((key) => same(at[key], to[key])) ? null : at;
}

function fail(message) {
  error.textContent = message;
  error.hidden = false;
  button.disabled = false;
}

const key = form.dataset.publishableKey;
let stripe = null;
let elements = null;
if (key && window.Stripe) {
  stripe = Stripe(key);
  elements = stripe.elements({
    mode: "payment",
    amount: Number(form.dataset.amount),
    currency: form.dataset.currency,
    appearance: { variables: { colorPrimary: "#e8606a", borderRadius: "6px" } },
  });
  // The details and the address are asked above: Stripe's form asks
  // nothing again (no second country), and they are handed to it below.
  // Link's "save my information" box would ask the e-mail, phone and name
  // a third time: left out too.
  elements
    .create("payment", { fields: { billingDetails: "never" }, wallets: { link: "never" } })
    .mount("#payment-element");
  const express = elements.create("expressCheckout", { buttonHeight: 44 });
  express.mount("#express-checkout");
  express.on("ready", ({ availablePaymentMethods }) => {
    if (availablePaymentMethods) document.querySelector("[data-express]").hidden = false;
  });
  express.on("click", (event) => event.resolve({
    emailRequired: true,
    shippingAddressRequired: true,
    // A whole address, else wallets hand only its country and postal code.
    billingAddressRequired: true,
    allowedShippingCountries: [...country.options].map((option) => option.value),
    // Before the wallet's address is known: those that ship to the form's
    // country, else all of them.
    shippingRates: rates.length
      ? (ratesTo(country.value).length ? ratesTo(country.value) : allRates)
      : [{ id: "standard", displayName: "Standard delivery", amount: 0 }],
  }));
  // Only the methods that ship to the wallet's address, priced in.
  express.on("shippingaddresschange", (event) => {
    if (!rates.length) { event.resolve(); return; }
    const here = ratesTo(event.address.country);
    if (!here.length) { event.reject(); return; }
    elements.update({ amount: goods + here[0].amount });
    event.resolve({ shippingRates: here });
  });
  express.on("shippingratechange", (event) => {
    if (rates.length) elements.update({ amount: goods + event.shippingRate.amount });
    event.resolve();
  });
  express.on("confirm", async (event) => {
    const billing = event.billingDetails || {};
    const shipping = event.shippingAddress || {};
    const address = shipping.address || {};
    const fill = (name, value) => { if (value) form.elements[name].value = value; };
    fill("email", billing.email);
    fill("name", shipping.name || billing.name);
    fill("phone", billing.phone);
    fill("line1", address.line1);
    fill("line2", address.line2);
    fill("postal_code", address.postal_code);
    fill("city", address.city);
    fill("country", address.country);
    // Billed elsewhere when the wallet says so; else at the delivery address.
    const other = walletBilling(billing, shipping);
    billingDifferent.checked = !!other;
    showBilling();
    if (other) {
      fill("billing_name", billing.name || shipping.name);
      for (const key of ["line1", "line2", "postal_code", "city", "country"]) {
        form.elements[`billing_${key}`].value = other[key] || "";
      }
    }
    const rate = methods.find((method) => method.value === event.shippingRate?.id);
    if (rate) rate.checked = true;
    showShipping();
    if (!(await pay())) event.paymentFailed({ reason: "fail" });
  });
}

// What Stripe's form was told not to ask, from the form's own fields: every
// one of them, as "never" requires.
// The address is the billing one when the shopper gave one.
function billingDetails() {
  const value = (name) => form.elements[name].value.trim();
  const at = billingDifferent.checked ? "billing_" : "";
  return {
    name: value(`${at}name`),
    email: value("email"),
    phone: value("phone"),
    address: {
      line1: value(`${at}line1`),
      line2: value(`${at}line2`),
      city: value(`${at}city`),
      postal_code: value(`${at}postal_code`),
      state: "",
      country: value(`${at}country`),
    },
  };
}

// Resolves false when the shopper stays on the page.
async function pay() {
  error.hidden = true;
  button.disabled = true;
  try {
    if (elements) {
      const { error: invalid } = await elements.submit();
      if (invalid) { fail(invalid.message); return false; }
    }
    const response = await fetch(form.dataset.startPath, {
      method: "POST",
      body: new URLSearchParams(new FormData(form)),
      headers: { accept: "application/json" },
    });
    const answer = await response.json();
    if (answer.redirect) { location.assign(answer.redirect); return true; }
    if (answer.error) { fail(answer.error); return false; }
    // Payments were set up while the page was open: it loads them.
    if (!stripe) { location.reload(); return true; }
    const { error: refused } = await stripe.confirmPayment({
      elements,
      clientSecret: answer.client_secret,
      confirmParams: {
        return_url: new URL(form.dataset.returnPath, location.href).href,
        payment_method_data: { billing_details: billingDetails() },
      },
    });
    // Only reached when the payment could not be confirmed; otherwise the
    // browser is on its way to the return page.
    fail(refused.message);
    return false;
  } catch {
    fail("Something went wrong. Please try again.");
    return false;
  }
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  pay();
});
"##;

#[topcoat::router::query_params(error = bad_request)]
struct CheckoutQuery {
    error: Option<String>,
    /// A saved address of the signed-in shopper to ship to.
    address: Option<String>,
    /// A saved address of the signed-in shopper to bill to.
    billing: Option<String>,
}

pub fn checkout_url(cx: &Cx, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        error: Option<&'a str>,
    }
    href!(index).query(Link { error }).resolve(cx)
}

/// The checkout, shipping and billing to saved addresses.
fn saved_url(cx: &Cx, address: Option<&str>, billing: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        address: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        billing: Option<&'a str>,
    }
    href!(index).query(Link { address, billing }).resolve(cx)
}

/// `incl. 9.97 EUR VAT FR 20%`.
fn vat_text(estimate: &VatEstimate) -> String {
    format!("incl. {} {}", estimate.amount(), estimate.label())
}

/// The shop's shipping options, as Express Checkout's rates (with the
/// countries each ships to): `[]` when shipping is included.
pub(crate) fn express_rates(cart: &CartView) -> String {
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

/// `Ada L.` as `AL`.
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CheckoutQuery>(cx)?;
    // Nothing to order (or something off sale to remove first): the cart.
    let cart = current_cart(cx)
        .await?
        .filter(|cart| cart.can_check_out())
        .ok_or_redirect(cart_url(cx, None))?;
    let nav = menu(cx).await?;
    let setup = payment_setup(cx).await?;
    let reviews = recent_reviews(cx, REVIEWS_SHOWN).await?;
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
    let sign_in = sign_in_url(cx, Some(&href!(index).resolve(cx)), None);
    let currency = cart.currency.clone().unwrap_or_default();
    let country = if draft.country.is_empty() {
        DEFAULT_COUNTRY.to_owned()
    } else {
        draft.country.to_ascii_uppercase()
    };
    let billing_country = if draft.billing_country.is_empty() {
        country.clone()
    } else {
        draft.billing_country.to_ascii_uppercase()
    };
    let bills_elsewhere = draft.bills_elsewhere();
    // The shipping method: the one picked last when it ships here, else the
    // first that does.
    let shipping = cart
        .shipping_options_to(&country)
        .find(|option| option.id == draft.shipping_method)
        .or_else(|| cart.shipping_options_to(&country).next())
        .cloned();
    let shipping_text = match &shipping {
        Some(option) => option.price(),
        None if cart.shipping_required => "—".to_owned(),
        None => "Included".to_owned(),
    };
    // What the VAT line says for each country and shipping method (`""`
    // for none), for the page's script to switch.
    let countries: Vec<(&str, &str, String)> = COUNTRIES
        .iter()
        .map(|(code, name)| {
            let mut texts = serde_json::Map::new();
            for option in std::iter::once(None).chain(cart.shipping_options_to(code).map(Some)) {
                if let Some(estimate) = cart.vat_estimate_with(code, option) {
                    texts.insert(
                        option.map(|option| option.id.clone()).unwrap_or_default(),
                        vat_text(&estimate).into(),
                    );
                }
            }
            (*code, *name, serde_json::Value::Object(texts).to_string())
        })
        .collect();
    let vat = cart
        .vat_estimate_with(&country, shipping.as_ref())
        .as_ref()
        .map(vat_text)
        .filter(|_| draft.vat_number.trim().is_empty() || country == "FR");
    let total_cents = cart.total_with(shipping.as_ref());
    let total = format_money(total_cents, &currency);
    let rates = express_rates(&cart);
    // The offers the shopper may add, among the products this site sells.
    let offers: Vec<OfferView> = cart
        .offers
        .iter()
        .filter(|offer| offer.slug.is_some())
        .cloned()
        .collect();
    let back_to_cart = href!(crate::app::cart::index).resolve(cx);
    let publishable_key = setup
        .as_ref()
        .map(|setup| setup.publishable_key.clone())
        .unwrap_or_default();

    Ok(view! {
        document(
            title: "Checkout",
            nav: &nav,
            no_drawer: true,
            <ol class="steps" aria-label="Checkout steps">
                <li class="done"><a href=(&back_to_cart)>"Cart"</a></li>
                <li aria-current="step">"Shipping & payment"</li>
                <li>"Confirmation"</li>
            </ol>
            <h1 class="vh">"Checkout"</h1>
            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            }
            <div class="checkout-grid">
                <aside class="checkout-aside" aria-label="Order summary">
                    <section class="card summary" data-checkout-summary="">
                        <h2>"Your cart"</h2>
                        <ul class="lines">
                            for line in &cart.lines {
                                summary_line(line: line, currency: &currency, back: "checkout")
                            }
                        </ul>
                        <details class="promo" open=(cart.discount_code.is_some())>
                            <summary>"Have a promo code? Enter it here"</summary>
                            <form class="discount" method="post" action=(href!(discount::discount))>
                                <input type="hidden" name="back" value="checkout">
                                if let Some(code) = &cart.discount_code {
                                    <span>"Code " <code>(code)</code></span>
                                    <button name="remove" value="1">"Remove the code"</button>
                                } else {
                                    <label>
                                        <span class="vh">"Discount code"</span>
                                        <input type="text" name="code" maxlength="32" autocomplete="off" placeholder="Discount code">
                                    </label>
                                    <button>"Apply"</button>
                                }
                            </form>
                            if let Some(refused) = cart.discount_refused {
                                <p class="notice" data-discount-refused=(refused)>(say(refused))</p>
                            }
                        </details>
                        <dl class="totals">
                            <dt>"Subtotal"</dt>
                            <dd data-checkout-subtotal="">(cart.subtotal().unwrap_or_default())</dd>
                            if let (Some(discount), Some(amount)) = (&cart.discount, cart.discount_amount()) {
                                <dt>"Discount " <code>(&discount.code)</code></dt>
                                <dd data-checkout-discount=(&discount.code)>(amount)</dd>
                            }
                            <dt>"Shipping"</dt>
                            <dd data-checkout-shipping="">(&shipping_text)</dd>
                            <dt class="total">"Total"</dt>
                            <dd class="total">
                                <strong class="price" data-checkout-total="">(&total)</strong>
                                <small data-checkout-vat="" hidden=(vat.is_none())>
                                    if let Some(vat) = &vat { "(" (vat) ")" }
                                </small>
                            </dd>
                        </dl>
                    </section>
                    <section class="reassure">
                        <h2>"Order with confidence"</h2>
                        <ul>
                            <li>"Secure payment by Stripe: your card never touches this shop."</li>
                            <li>"An invoice with every order."</li>
                            <li>"14 days to change your mind."</li>
                        </ul>
                    </section>
                    if !reviews.is_empty() {
                        <section class="reviews-aside" data-checkout-reviews="">
                            <h2>"Our customers say"</h2>
                            for review in &reviews {
                                <figure class="review">
                                    <figcaption>
                                        <span class="avatar" aria-hidden="true">(initials(&review.author_name))</span>
                                        <span>(&review.author_name)</span>
                                    </figcaption>
                                    <div class="rating" aria-label=(format!("{} out of 5", review.rating))>(review.stars())</div>
                                    if let Some(title) = &review.title { <strong>(title)</strong> }
                                    <blockquote>(&review.body)</blockquote>
                                </figure>
                            }
                        </section>
                    }
                </aside>

                <div class="checkout-main">
                    if let Some(shopper) = &shopper {
                        <p class="returning" data-signed-in="">"Signed in as " <strong>(&shopper.email)</strong></p>
                    } else if accounts {
                        <p class="returning" data-returning="">
                            "Already a customer? " <a href=(&sign_in)>"Click here to log in"</a>
                        </p>
                    }
                    if setup.is_some() {
                        <section class="express" data-express="" hidden="" aria-label="Express checkout">
                            <h2>"Express checkout"</h2>
                            <div id="express-checkout"></div>
                            <p class="or"><span>"or"</span></p>
                        </section>
                    }
                    <form class="checkout" method="post" action=(href!(place)) data-checkout-form=""
                        data-start-path=(href!(start::start)) data-return-path=(href!(complete::index))
                        data-publishable-key=(&publishable_key) data-amount=(total_cents)
                        data-goods=(cart.total_cents) data-shipping=(&rates)
                        data-currency=(currency.to_lowercase())>
                        <input type="hidden" name="expected_total_cents" value=(total_cents)>
                        <section class="card">
                            <h2>"Your details"</h2>
                            <label for="checkout-email">"E-mail"</label>
                            <input id="checkout-email" name="email" type="email" required="" autocomplete="email" value=(&draft.email) readonly=(shopper.is_some())>
                            <label for="checkout-name">"Full name"</label>
                            <input id="checkout-name" name="name" required="" autocomplete="name" value=(&draft.name)>
                            <label for="checkout-phone">"Phone (optional)"</label>
                            <input id="checkout-phone" name="phone" type="tel" autocomplete="tel" value=(&draft.phone)>
                        </section>
                        <section class="card">
                            <h2>"Delivery address"</h2>
                            if let Some(shopper) = shopper.as_ref().filter(|shopper| !shopper.addresses.is_empty()) {
                                <ul class="saved" data-saved-addresses="">
                                    for saved in &shopper.addresses {
                                        <li data-saved-address=(&saved.address.city) data-current=(shipping_to.as_deref() == Some(saved.id.as_str()))
                                            data-billing-current=(billing_to.as_deref() == Some(saved.id.as_str()))>
                                            (&saved.address.name) ", " (&saved.address.line1) ", "
                                            (&saved.address.postal_code) " " (&saved.address.city) " "
                                            <a href=(saved_url(cx, Some(&saved.id), billing_to.as_deref()))>"Ship here"</a> " "
                                            <a href=(saved_url(cx, shipping_to.as_deref(), Some(&saved.id))) data-bill-here="">"Bill here"</a>
                                        </li>
                                    }
                                </ul>
                            }
                            <label for="checkout-line1">"Street address"</label>
                            <input id="checkout-line1" name="line1" required="" autocomplete="address-line1" value=(&draft.line1)>
                            <details class="more" open=(!draft.line2.is_empty())>
                                <summary>"+ Add a company, apartment, floor…"</summary>
                                <label for="checkout-line2">"Apartment, suite (optional)"</label>
                                <input id="checkout-line2" name="line2" autocomplete="address-line2" value=(&draft.line2)>
                            </details>
                            <div class="row">
                                <span>
                                    <label for="checkout-city">"City"</label>
                                    <input id="checkout-city" name="city" required="" autocomplete="address-level2" value=(&draft.city)>
                                </span>
                                <span>
                                    <label for="checkout-postal-code">"Postal code"</label>
                                    <input id="checkout-postal-code" name="postal_code" required="" autocomplete="postal-code" value=(&draft.postal_code)>
                                </span>
                                <span>
                                    <label for="checkout-country">"Country"</label>
                                    <select id="checkout-country" name="country" required="" autocomplete="country">
                                        for (code, name, text) in &countries {
                                            <option value=(*code) selected=(*code == country) data-vat=(text)>(*name)</option>
                                        }
                                    </select>
                                </span>
                            </div>
                            if !cart.shipping_options.is_empty() || cart.shipping_required {
                                <fieldset class="shipping-methods" data-shipping-methods="">
                                    <legend>"Delivery method"</legend>
                                    for option in &cart.shipping_options {
                                        <label class="method" hidden=(!option.ships_to(&country))>
                                            <input type="radio" name="shipping_method" value=(&option.id)
                                                data-amount=(option.amount_cents) data-price=(option.price())
                                                data-countries=(option.countries.join(" "))
                                                checked=(shipping.as_ref().is_some_and(|picked| picked.id == option.id))
                                                disabled=(!option.ships_to(&country))>
                                            <span class="name">(&option.name)</span>
                                            <span class="price">(option.price())</span>
                                        </label>
                                    }
                                    <p class="notice" data-no-shipping="" hidden=(shipping.is_some())>"This shop does not deliver to this country."</p>
                                </fieldset>
                            }
                            if business {
                                <label for="checkout-vat">"EU company VAT number (optional, for an invoice without VAT)"</label>
                                <input id="checkout-vat" name="vat_number" autocomplete="off" placeholder="DE123456789" value=(&draft.vat_number)>
                            }
                            <label class="check">
                                <input type="checkbox" name="billing_different" value="1" checked=(bills_elsewhere)>
                                " Use a different billing address (optional)"
                            </label>
                            <fieldset class="billing" data-billing="" hidden=(!bills_elsewhere) disabled=(!bills_elsewhere)>
                                <legend class="vh">"Billing address"</legend>
                                <label for="billing-name">"Billing name or company"</label>
                                <input id="billing-name" name="billing_name" required="" autocomplete="billing name" value=(&draft.billing_name)>
                                <label for="billing-line1">"Billing street address"</label>
                                <input id="billing-line1" name="billing_line1" required="" autocomplete="billing address-line1" value=(&draft.billing_line1)>
                                <label for="billing-line2">"Billing apartment, suite (optional)"</label>
                                <input id="billing-line2" name="billing_line2" autocomplete="billing address-line2" value=(&draft.billing_line2)>
                                <div class="row">
                                    <span>
                                        <label for="billing-city">"Billing city"</label>
                                        <input id="billing-city" name="billing_city" required="" autocomplete="billing address-level2" value=(&draft.billing_city)>
                                    </span>
                                    <span>
                                        <label for="billing-postal-code">"Billing postal code"</label>
                                        <input id="billing-postal-code" name="billing_postal_code" required="" autocomplete="billing postal-code" value=(&draft.billing_postal_code)>
                                    </span>
                                    <span>
                                        <label for="billing-country">"Billing country"</label>
                                        <select id="billing-country" name="billing_country" required="" autocomplete="billing country">
                                            for (code, name) in COUNTRIES {
                                                <option value=(*code) selected=(*code == billing_country)>(*name)</option>
                                            }
                                        </select>
                                    </span>
                                </div>
                                if shopper.is_some() {
                                    <label class="check">
                                        <input type="checkbox" name="save_billing_address" value="1">
                                        " Save this billing address to my account"
                                    </label>
                                }
                            </fieldset>
                            <p class="hint" data-import-notice="">"Shipping outside the EU? Import duties and taxes may be due on delivery."</p>
                            if shopper.is_some() {
                                <label class="check">
                                    <input type="checkbox" name="save_address" value="1" checked="">
                                    " Save this address to my account"
                                </label>
                            }
                            <label for="checkout-note">"Note for the shop (optional)"</label>
                            <textarea id="checkout-note" name="note" rows="2">(&draft.note)</textarea>
                        </section>
                        if !offers.is_empty() {
                            <section class="card offers" aria-labelledby="checkout-offers-title" data-checkout-offers="">
                                <h2 id="checkout-offers-title">"Exclusive offer"</h2>
                                <ul>
                                    for offer in &offers {
                                        <li data-checkout-offer=(&offer.name)>
                                            if let Some(url) = &offer.image_url {
                                                <img src=(url) alt="" width="56" height="56" loading="lazy">
                                            } else {
                                                <span></span>
                                            }
                                            <span class="what">
                                                <strong>(offer.heading())</strong>
                                                if offer.title.is_some() { <small>(&offer.name)</small> }
                                                if let Some(label) = &offer.variant_label { <small class="variant">(label)</small> }
                                                <span>
                                                    <s aria-label=(format!("instead of {}", offer.price()))>(offer.price())</s> " "
                                                    <strong class="price" data-offer-price="">(offer.offer_price())</strong> " "
                                                    <span class="off">"−" (offer.percent_off()) "%"</span>
                                                </span>
                                            </span>
                                            // The offer's own form sits after this one: forms do not nest.
                                            <button type="submit" form=(format!("offer-{}", offer.offer_id))
                                                aria-label=(format!("Add {} at {}", offer.name, offer.offer_price()))>"Add"</button>
                                        </li>
                                    }
                                </ul>
                            </section>
                        }
                        <section class="card">
                            <h2>"Payment"</h2>
                            <p class="hint">"Every transaction is secured and encrypted. Card details are never stored on our servers."</p>
                            if setup.is_some() {
                                <div id="payment-element"></div>
                            } else {
                                <p class="hint" data-payments-off="">"Card payments are not available in this shop yet."</p>
                            }
                            <p class="notice" role="alert" data-payment-error="" hidden=""></p>
                            <button class="primary place">"Place order · " <span data-place-total="">(&total)</span></button>
                        </section>
                    </form>
                    for offer in &offers {
                        <form id=(format!("offer-{}", offer.offer_id)) method="post" action=(href!(crate::app::cart::add::add))>
                            <input type="hidden" name="slug" value=(offer.slug.as_deref().unwrap_or_default())>
                            <input type="hidden" name="variant_id" value=(offer.variant_id.as_deref().unwrap_or_default())>
                            <input type="hidden" name="quantity" value="1">
                            <input type="hidden" name="offer" value=(&offer.offer_id)>
                            <input type="hidden" name="back" value="checkout">
                        </form>
                    }
                </div>
            </div>
            if setup.is_some() {
                <script src="https://js.stripe.com/v3/"></script>
            }
            <script>(Unescaped::new_unchecked(CHECKOUT_SCRIPT))</script>
        )
    })
}

#[derive(serde::Deserialize)]
pub struct CheckoutForm {
    // A string: `flatten` hands every field to serde as text.
    expected_total_cents: String,
    /// Signed in: keep the address on the account.
    #[serde(default)]
    save_address: String,
    /// Signed in: keep the billing address on the account too.
    #[serde(default)]
    save_billing_address: String,
    #[serde(flatten)]
    draft: CheckoutDraft,
}

/// How a request to pay ended.
pub(crate) enum Asked {
    /// A payment is open; the browser confirms it with this secret.
    Open(String),
    /// Refused, with the reason; the cart is where some are fixed.
    Refused {
        code: Option<&'static str>,
        to_cart: bool,
    },
}

impl Asked {
    /// The page a refusal sends the shopper back to.
    pub(crate) fn back(&self, cx: &Cx) -> String {
        match self {
            Asked::Refused {
                code,
                to_cart: true,
            } => cart_url(cx, *code),
            Asked::Refused { code, .. } => checkout_url(cx, *code),
            Asked::Open(_) => checkout_url(cx, None),
        }
    }
}

/// Asks to pay for the cart. The details are kept first, so a refused form
/// comes back filled in; the payment opened for it is remembered for the
/// return page.
pub(crate) async fn ask(cx: &Cx, form: CheckoutForm) -> Result<Asked> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(Asked::Refused {
            code: None,
            to_cart: true,
        });
    };
    let mut draft = form.draft;
    // A signed-in shopper orders under their account's address, whatever
    // the form said.
    let token = shopper_token(cx).filter(|_| accounts_enabled(cx));
    if token.is_some()
        && let Some(shopper) = signed_in(cx).await
    {
        draft.email = shopper.email.clone();
    }
    save_checkout_draft(cx, draft.clone())?;
    let business = accepts_business_buyers(cx).await?;
    let checkout = Checkout {
        email: draft.email.clone(),
        shipping: draft.shipping(),
        note: Some(draft.note.clone()),
        // Unreadable means not shown: treat it as a price that moved.
        expected_total_cents: Some(form.expected_total_cents.trim().parse().unwrap_or(u64::MAX)),
        // A number kept from before the shop stopped selling to businesses
        // is not sent: the field is not shown any more.
        vat_number: business.then(|| draft.vat_number.clone()),
        billing: draft.billing(),
        shipping_method: Some(draft.shipping_method.clone()),
        // The shop mails about the order in the shopper's language.
        language: request_language(cx),
        // A scheduled newsletter keeps the shopper's hours.
        time_zone: request_time_zone(cx),
        // This example has no newsletter to sign up to.
        newsletter: false,
    };
    let err = match start_payment(cx, &cart_id, checkout).await? {
        Ok(session) => {
            // The addresses checked out: kept on the account for the next
            // order. A refusal (ten kept already) does not stop the payment.
            // The billing one first: the latest saved is the next delivery.
            if let Some(token) = &token {
                if let Some(billing) = draft
                    .billing()
                    .filter(|_| !form.save_billing_address.is_empty())
                {
                    let _ = save_address(cx, token, &billing).await?;
                }
                if !form.save_address.is_empty() {
                    let _ = save_address(cx, token, &draft.shipping()).await?;
                }
            }
            set_payment_in_progress(
                cx,
                PaymentInProgress {
                    cart_id,
                    request_id: session.request_id,
                },
            )?;
            return Ok(Asked::Open(session.client_secret));
        }
        Err(err) => err,
    };
    let code = err.code();
    let to_cart = matches!(err, OrdersError::EmptyCart | OrdersError::Unavailable { .. })
        // The cart is where the code is changed or taken off.
        || code.is_some_and(|code| code.starts_with("discount_"));
    Ok(Asked::Refused { code, to_cart })
}

/// The form sent without the page's script: Stripe's form needs it, so a
/// payment cannot be confirmed this way; a refusal is still worded.
#[route(POST)]
pub async fn place(cx: &Cx, Form(form): Form<CheckoutForm>) -> Result<SeeOther> {
    let asked = ask(cx, form).await?;
    Ok(see_other(asked.back(cx)))
}
