//! `/checkout`: one page from the cart to the payment. The shopper's
//! details, the address and Stripe's Payment Element sit side by side with
//! the cart, which can still be changed there. Stripe's form shows before
//! any payment is open (its "deferred" mode): the button asks to pay
//! (`/checkout/start`), which opens the payment for the cart as it stands, then
//! confirms it in the browser, which comes back to `/checkout/complete`. The
//! total the page showed travels with the request, so a price the shopper
//! did not see is never charged.

pub mod complete;
pub mod countries;
pub mod pay;
pub mod start;

use timada_storefront::{
    Checkout, OrdersError, VatEstimate, categories, payment_setup, recent_reviews, start_payment,
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

use crate::app::cart::{cart_url, current_cart, discount, summary_line};
use crate::app::{document, say};
use crate::shopper::{
    CheckoutDraft, PaymentInProgress, cart_id, checkout_draft, save_checkout_draft,
    set_payment_in_progress,
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
const vatNumber = form.elements.vat_number;

function showVat() {
  if (!vat) return;
  const text = country.selectedOptions[0]?.dataset.vat;
  const reverseCharge = vatNumber.value.trim() !== "" && country.value !== "FR";
  vat.hidden = !text || reverseCharge;
  vat.textContent = text ? `(${text})` : "";
}
country.addEventListener("change", showVat);
vatNumber.addEventListener("input", showVat);

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
    allowedShippingCountries: [...country.options].map((option) => option.value),
    shippingRates: [{ id: "standard", displayName: "Standard delivery", amount: 0 }],
  }));
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
    if (!(await pay())) event.paymentFailed({ reason: "fail" });
  });
}

// What Stripe's form was told not to ask, from the form's own fields: every
// one of them, as "never" requires.
function billingDetails() {
  const value = (name) => form.elements[name].value.trim();
  return {
    name: value("name"),
    email: value("email"),
    phone: value("phone"),
    address: {
      line1: value("line1"),
      line2: value("line2"),
      city: value("city"),
      postal_code: value("postal_code"),
      state: "",
      country: country.value,
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
}

pub fn checkout_url(cx: &Cx, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        error: Option<&'a str>,
    }
    href!(index).query(Link { error }).resolve(cx)
}

/// `incl. 9.97 EUR VAT FR 20%`.
fn vat_text(estimate: &VatEstimate) -> String {
    format!("incl. {} {}", estimate.amount(), estimate.label())
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
    let nav = categories(cx).await?;
    let setup = payment_setup(cx).await?;
    let reviews = recent_reviews(cx, REVIEWS_SHOWN).await?;
    let draft = checkout_draft(cx);
    let currency = cart.currency.clone().unwrap_or_default();
    let country = if draft.country.is_empty() {
        DEFAULT_COUNTRY.to_owned()
    } else {
        draft.country.to_ascii_uppercase()
    };
    // What the VAT line says for each country, for the select to switch.
    let countries: Vec<(&str, &str, Option<String>)> = COUNTRIES
        .iter()
        .map(|(code, name)| (*code, *name, cart.vat_estimate(code).as_ref().map(vat_text)))
        .collect();
    let vat = countries
        .iter()
        .find(|(code, ..)| *code == country)
        .and_then(|(.., text)| text.clone())
        .filter(|_| draft.vat_number.trim().is_empty() || country == "FR");
    let total = cart.total().unwrap_or_default();
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
                            <dd>"Included"</dd>
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
                    if setup.is_some() {
                        <section class="express" data-express="" hidden="" aria-label="Express checkout">
                            <h2>"Express checkout"</h2>
                            <div id="express-checkout"></div>
                            <p class="or"><span>"or"</span></p>
                        </section>
                    }
                    <form class="checkout" method="post" action=(href!(place)) data-checkout-form=""
                        data-start-path=(href!(start::start)) data-return-path=(href!(complete::index))
                        data-publishable-key=(&publishable_key) data-amount=(cart.total_cents)
                        data-currency=(currency.to_lowercase())>
                        <input type="hidden" name="expected_total_cents" value=(cart.total_cents)>
                        <section class="card">
                            <h2>"Your details"</h2>
                            <label for="checkout-email">"E-mail"</label>
                            <input id="checkout-email" name="email" type="email" required="" autocomplete="email" value=(&draft.email)>
                            <label for="checkout-name">"Full name"</label>
                            <input id="checkout-name" name="name" required="" autocomplete="name" value=(&draft.name)>
                            <label for="checkout-phone">"Phone (optional)"</label>
                            <input id="checkout-phone" name="phone" type="tel" autocomplete="tel" value=(&draft.phone)>
                        </section>
                        <section class="card">
                            <h2>"Delivery address"</h2>
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
                                            <option value=(*code) selected=(*code == country) data-vat=(text.as_deref().unwrap_or(""))>(*name)</option>
                                        }
                                    </select>
                                </span>
                            </div>
                            <label for="checkout-vat">"EU company VAT number (optional, for an invoice without VAT)"</label>
                            <input id="checkout-vat" name="vat_number" autocomplete="off" placeholder="DE123456789" value=(&draft.vat_number)>
                            <p class="hint" data-import-notice="">"Shipping outside the EU? Import duties and taxes may be due on delivery."</p>
                            <label for="checkout-note">"Note for the shop (optional)"</label>
                            <textarea id="checkout-note" name="note" rows="2">(&draft.note)</textarea>
                        </section>
                        <section class="card">
                            <h2>"Payment"</h2>
                            <p class="hint">"Every transaction is secured and encrypted. Card details are never stored on our servers."</p>
                            if setup.is_some() {
                                <div id="payment-element"></div>
                            } else {
                                <p class="hint" data-payments-off="">"Card payments are not available in this shop yet."</p>
                            }
                            <p class="notice" role="alert" data-payment-error="" hidden=""></p>
                            <button class="primary place">"Place order · " (&total)</button>
                        </section>
                    </form>
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
    let draft = form.draft;
    save_checkout_draft(cx, draft.clone())?;
    let checkout = Checkout {
        email: draft.email.clone(),
        shipping: draft.shipping(),
        note: Some(draft.note.clone()),
        // Unreadable means not shown: treat it as a price that moved.
        expected_total_cents: Some(form.expected_total_cents.trim().parse().unwrap_or(u64::MAX)),
        vat_number: Some(draft.vat_number.clone()),
    };
    let err = match start_payment(cx, &cart_id, checkout).await? {
        Ok(session) => {
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
