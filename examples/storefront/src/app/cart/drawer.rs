//! The cart drawer: the cart sliding in from the right on every page, with
//! what goes well with it beside. Without the page's script it opens on the
//! `#cart` fragment the cart forms come back to; with it, nothing reloads
//! (see `CART_SCRIPT` in the layout).

use timada_storefront::{
    CartView, Product, cart_recommendations, payment_setup, price, product_image,
};
use topcoat::{
    Result,
    context::Cx,
    router::{href, query_params, request::original_uri},
    view::{View, attributes, view},
};

use super::{add::add, discount, drawer_url, summary_line};
use crate::app::checkout::countries::COUNTRIES;
use crate::app::say;

/// How many products the "frequently bought together" column offers.
const OFFERED: u32 = 6;

#[topcoat::router::query_params(error = bad_request)]
struct DrawerQuery {
    /// A refusal from a form sent from the drawer, by code.
    cart_error: Option<String>,
}

/// The cart drawer, closed until the `#cart` fragment or the page's script
/// opens it. `cart` is this browser's cart, if it has one.
#[topcoat::view::component]
pub async fn cart_drawer(cx: &Cx, #[default] cart: Option<&CartView>) -> Result<impl View> {
    let query = query_params::<DrawerQuery>(cx)?;
    // Where the drawer's forms come back to: this page, as the shopper sees
    // it, without the refusal it may already show.
    let from = original_uri(cx)
        .path_and_query()
        .map(|path| path.as_str())
        .unwrap_or("/");
    let from = drawer_url(from, None).trim_end_matches("#cart").to_owned();
    let cart = cart.filter(|cart| !cart.is_empty()).cloned();
    let offered: Vec<Product> = match &cart {
        Some(cart) => cart_recommendations(cx, cart, OFFERED).await?,
        None => Vec::new(),
    };
    let setup = match &cart {
        Some(cart) if cart.can_check_out() => payment_setup(cx).await?,
        _ => None,
    };
    let count = cart
        .as_ref()
        .map(|cart| cart.item_count)
        .unwrap_or_default();
    let currency = cart
        .as_ref()
        .and_then(|cart| cart.currency.clone())
        .unwrap_or_default();
    let total = cart
        .as_ref()
        .and_then(|cart| cart.total())
        .unwrap_or_default();
    let total_cents = cart
        .as_ref()
        .map(|cart| cart.total_cents)
        .unwrap_or_default();
    let publishable_key = setup
        .as_ref()
        .map(|setup| setup.publishable_key.clone())
        .unwrap_or_default();
    let countries = COUNTRIES
        .iter()
        .map(|(code, _)| *code)
        .collect::<Vec<_>>()
        .join(",");
    let rates = cart
        .as_ref()
        .map(crate::app::checkout::express_rates)
        .unwrap_or_else(|| "[]".to_owned());
    let error = query.cart_error.clone();
    // A refused code leaves the promo box open, to type it again.
    let promo_refused = error
        .as_deref()
        .is_some_and(|code| code.starts_with("discount_"));

    Ok(view! {
        <div id="cart" class="drawer" data-cart-drawer="" data-count=(count)
            data-publishable-key=(&publishable_key) data-amount=(total_cents)
            data-currency=(currency.to_lowercase()) data-countries=(&countries) data-shipping=(&rates)
            data-start-path=(href!(crate::app::checkout::start::start))
            data-return-path=(href!(crate::app::checkout::complete::index))>
            <a class="drawer-backdrop" href="#" tabindex="-1" aria-hidden="true" data-drawer-close=""></a>
            <div class="drawer-sheet">
                <section class="drawer-panel" role="dialog" aria-labelledby="cart-drawer-title">
                    <header class="drawer-head">
                        <h2 id="cart-drawer-title">"Your cart (" (count) ")"</h2>
                        <a class="drawer-close" href="#" aria-label="Close the cart" data-drawer-close="">"×"</a>
                    </header>
                    if let Some(code) = error.as_deref() {
                        <p class="notice" role="alert" data-cart-error=(code)>(say(code))</p>
                    }
                    if let Some(cart) = &cart {
                        <div class="summary">
                            <ul class="lines">
                                for line in cart.lines.iter().chain(&cart.unavailable) {
                                    summary_line(line: line, currency: &currency, back: "drawer", from: &from)
                                }
                            </ul>
                            <details class="promo" open=(cart.discount_code.is_some() || promo_refused)>
                                <summary>"Have a promo code?"</summary>
                                <form class="discount" method="post" action=(href!(discount::discount))>
                                    <input type="hidden" name="back" value="drawer">
                                    <input type="hidden" name="from" value=(&from)>
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
                                <dd data-drawer-subtotal="">(cart.subtotal().unwrap_or_default())</dd>
                                if let (Some(discount), Some(amount)) = (&cart.discount, cart.discount_amount()) {
                                    <dt>"Discount " <code>(&discount.code)</code></dt>
                                    <dd data-drawer-discount=(&discount.code)>(amount)</dd>
                                }
                                <dt class="total">"Total"</dt>
                                <dd class="total"><strong class="price" data-drawer-total="">(&total)</strong></dd>
                            </dl>
                            <p class="hint">"Shipping and taxes are worked out at checkout."</p>
                        </div>
                        if setup.is_some() {
                            <div class="drawer-express" data-drawer-express="" hidden="">
                                <div id="drawer-express-checkout"></div>
                                <p class="notice" role="alert" data-drawer-pay-error="" hidden=""></p>
                            </div>
                        }
                        if cart.can_check_out() {
                            <a class="place" href=(href!(crate::app::checkout::index))>"Check out · " (&total)</a>
                        }
                        <a class="drawer-continue" href="#" data-drawer-close="">"Continue shopping"</a>
                    } else {
                        <p class="drawer-empty" data-cart-empty="">"Your cart is empty."</p>
                        <a class="drawer-continue" href=(href!(crate::app::index))>"Back to the catalog"</a>
                    }
                </section>
                if !offered.is_empty() {
                    <section class="upsell" aria-labelledby="cart-upsell-title" data-upsell="">
                        <h2 id="cart-upsell-title">"Frequently bought together"</h2>
                        <ul>
                            for product in &offered {
                                <li data-offer=(&product.slug)>
                                    <a href=(href!(crate::app::p::slug::index, crate::app::p::slug::Slug(product.slug.clone())))>
                                        if let Some(image) = product.image() {
                                            product_image(image: image, attrs: attributes! { class="offer-pic" })
                                        }
                                        <span>(&product.name)</span>
                                    </a>
                                    price(product: product, attrs: attributes! { class="price" })
                                    if product.options.is_empty() {
                                        <form method="post" action=(href!(add))>
                                            <input type="hidden" name="slug" value=(&product.slug)>
                                            <input type="hidden" name="quantity" value="1">
                                            <input type="hidden" name="from" value=(&from)>
                                            <button aria-label=(format!("Add {}", product.name))>"Add"</button>
                                        </form>
                                    } else {
                                        <a class="offer-options" href=(href!(crate::app::p::slug::index, crate::app::p::slug::Slug(product.slug.clone())))
                                            aria-label=(format!("Choose the options of {}", product.name))>"Choose options"</a>
                                    }
                                </li>
                            }
                        </ul>
                    </section>
                }
            </div>
        </div>
    })
}

/// The drawer's look, in the shop's coral and ink. It shows on the `#cart`
/// fragment (no script: the page loads with it open, no slide) or slides in
/// on the `open` class (the script, which adds `shut` to slide it out while
/// the fragment is still there).
pub const DRAWER_CSS: &str = r#"
.cart-link { position: relative; text-decoration: none; color: #3d3b5c; font-weight: 600; }
.cart-link .count { display: inline-block; min-width: 1.3rem; height: 1.3rem; line-height: 1.3rem; margin-left: .3rem; border-radius: 1rem; background: #e8606a; color: #fff; font-size: .75rem; text-align: center; }
.drawer { position: fixed; inset: 0; z-index: 50; display: flex; justify-content: flex-end; visibility: hidden; pointer-events: none; }
.drawer.open, .drawer:target:not(.shut) { visibility: visible; pointer-events: auto; }
.drawer.shut { transition: visibility 0s linear .3s; }
.drawer-backdrop { position: absolute; inset: 0; background: rgba(61,59,92,.4); opacity: 0; }
.drawer.open .drawer-backdrop, .drawer:target:not(.shut) .drawer-backdrop { opacity: 1; }
.drawer-sheet { position: relative; display: flex; height: 100%; max-width: 100%; transform: translateX(100%); box-shadow: -8px 0 24px rgba(0,0,0,.12); }
.drawer.open .drawer-sheet, .drawer:target:not(.shut) .drawer-sheet { transform: none; }
.drawer.open .drawer-sheet, .drawer.shut .drawer-sheet { transition: transform .3s ease; }
.drawer.open .drawer-backdrop, .drawer.shut .drawer-backdrop { transition: opacity .3s; }
.drawer-panel { order: 2; width: 26rem; max-width: 100vw; box-sizing: border-box; height: 100%; overflow-y: auto; background: #fff; padding: 1rem 1.25rem 1.5rem; display: flex; flex-direction: column; gap: .75rem; }
.drawer-head { display: flex; justify-content: space-between; align-items: center; margin: 0; border-bottom: 1px solid #eee; padding-bottom: .75rem; }
.drawer-head h2 { margin: 0; font-size: 1rem; font-weight: 600; color: #3d3b5c; }
.drawer-close { font-size: 1.6rem; line-height: 1; text-decoration: none; color: #3d3b5c; padding: 0 .25rem; }
.drawer .summary ul.lines li { border-bottom: 1px solid #f3f3f3; }
.drawer .summary .what a { color: #3d3b5c; text-decoration: none; }
.drawer .summary .what small { display: block; color: #e8606a; font-size: .8rem; }
.drawer .summary .qty button { cursor: pointer; }
.drawer .summary .qty button:disabled { cursor: default; opacity: .4; }
.drawer .summary details.promo { margin: .75rem 0; }
.drawer .hint { margin: .5rem 0 0; font-size: .8rem; }
.drawer a.place { display: block; text-align: center; text-decoration: none; padding: .9rem; border-radius: 2rem; background: #f28b8f; color: #fff; font-weight: 600; letter-spacing: .03em; text-transform: uppercase; box-shadow: 0 6px 16px rgba(242,139,143,.35); }
.drawer a.place:hover { background: #e8606a; }
.drawer-continue { display: block; text-align: center; color: #3d3b5c; font-size: .9rem; }
.drawer-empty { color: #666; text-align: center; margin: 2rem 0 1rem; }
.drawer-express { margin: 0; }
.upsell { order: 1; width: 11rem; height: 100%; box-sizing: border-box; overflow-y: auto; background: #fbf5f5; padding: 1rem .75rem; border-right: 1px solid #eee; }
.upsell h2 { font-size: .85rem; font-weight: 600; color: #3d3b5c; text-align: center; margin: 0 0 1rem; }
.upsell ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 1.25rem; }
.upsell li { text-align: center; font-size: .8rem; color: #3d3b5c; }
.upsell li > a { display: block; color: inherit; text-decoration: none; }
.upsell .offer-pic { display: block; width: 4.5rem; height: 4.5rem; object-fit: cover; margin: 0 auto .4rem; border-radius: .4rem; background: #fff; }
.upsell .price { display: block; margin: .2rem 0 .4rem; font-weight: 400; color: #777; }
.upsell form { margin: 0; }
.upsell button, .upsell .offer-options { border: 1px solid #f28b8f; background: #fff; color: #e8606a; border-radius: .3rem; padding: .25rem .9rem; font-size: .75rem; cursor: pointer; text-decoration: none; display: inline-block; }
.upsell button:hover, .upsell .offer-options:hover { background: #f28b8f; color: #fff; }
@media (max-width: 899px) {
  .drawer-sheet { flex-direction: column; width: 26rem; }
  .drawer-panel { order: 1; flex: 1; width: 100%; }
  .upsell { order: 2; width: 100%; height: auto; flex: none; border-right: 0; border-top: 1px solid #eee; padding: .75rem; }
  .upsell h2 { text-align: left; margin-bottom: .5rem; }
  .upsell ul { flex-direction: row; overflow-x: auto; gap: 1rem; scroll-snap-type: x mandatory; }
  .upsell li { flex: 0 0 7rem; scroll-snap-align: start; }
}
@media (prefers-reduced-motion: reduce) {
  .drawer, .drawer-backdrop, .drawer-sheet { transition: none; }
}
"#;

/// Opens and closes the drawer, sends its forms (and the product page's add
/// form) without leaving the page, and offers Apple Pay / Google Pay from it.
/// A form's answer is the page it comes back to, rendered whole: the new
/// drawer is taken from it. Static and trusted: everything it needs comes
/// from the drawer's `data-` attributes.
pub const CART_SCRIPT: &str = r##"
(() => {
let drawer = document.querySelector("[data-cart-drawer]");
if (!drawer) return;
const link = document.querySelector("[data-cart-link]");
let opener = null;

function open() {
  if (!drawer.classList.contains("open")) opener = document.activeElement;
  drawer.classList.remove("shut");
  drawer.classList.add("open");
  drawer.querySelector(".drawer-close")?.focus({ preventScroll: true });
  express();
}
function close() {
  drawer.classList.remove("open");
  drawer.classList.add("shut");
  (opener || link)?.focus({ preventScroll: true });
}
function clearHash() {
  const url = new URL(location.href);
  url.searchParams.delete("cart_error");
  url.hash = "";
  history.replaceState(history.state, "", url);
}

link?.addEventListener("click", (event) => { event.preventDefault(); open(); });
document.addEventListener("click", (event) => {
  if (event.target.closest("[data-drawer-close]") && drawer.contains(event.target)) {
    event.preventDefault();
    close();
  }
});
document.addEventListener("keydown", (event) => {
  if (!drawer.classList.contains("open")) return;
  if (event.key === "Escape") { close(); return; }
  if (event.key !== "Tab") return;
  // Keep the focus in the drawer while it is open.
  const focusable = [...drawer.querySelectorAll("a[href], button:not([disabled]), input, summary, iframe")]
    .filter((el) => el.tabIndex >= 0 && el.offsetParent !== null);
  if (!focusable.length) return;
  const first = focusable[0], last = focusable[focusable.length - 1];
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
});
if (location.hash === "#cart") { open(); clearHash(); }

// The drawer of a page rendered by the server replaces this one; Stripe's
// buttons, already mounted, move across.
function swap(next) {
  const label = document.activeElement?.getAttribute("aria-label");
  const mounted = drawer.querySelector("[data-drawer-express]");
  const slot = next.querySelector("[data-drawer-express]");
  if (mounted && slot && stripeElements) slot.replaceWith(mounted);
  // Nothing to pay for any more: mount afresh when there is again.
  else if (!slot) stripeElements = null;
  next.classList.add("open");
  drawer.replaceWith(next);
  drawer = next;
  const count = document.querySelector("[data-cart-count]");
  if (count) {
    count.textContent = drawer.dataset.count;
    count.hidden = drawer.dataset.count === "0";
  }
  if (stripeElements && Number(drawer.dataset.amount) > 0) {
    stripeElements.update({ amount: Number(drawer.dataset.amount) });
  }
  const again = label && [...drawer.querySelectorAll("[aria-label]")].find((el) => el.getAttribute("aria-label") === label);
  (again || drawer.querySelector(".drawer-close"))?.focus({ preventScroll: true });
  express();
}

document.addEventListener("submit", async (event) => {
  const form = event.target;
  const inDrawer = drawer.contains(form);
  if (!inDrawer && !form.matches("form.picker")) return;
  event.preventDefault();
  const button = event.submitter;
  const body = new URLSearchParams(new FormData(form, button));
  if (!inDrawer) body.set("from", location.pathname + location.search);
  const buttons = [...form.querySelectorAll("button:not([disabled])")];
  buttons.forEach((b) => { b.disabled = true; });
  try {
    const response = await fetch(form.action, { method: "POST", body });
    const url = new URL(response.url);
    // A refusal on the product page is shown there.
    if (url.searchParams.has("error")) { location.assign(url); return; }
    const page = new DOMParser().parseFromString(await response.text(), "text/html");
    const next = page.querySelector("[data-cart-drawer]");
    if (!next) { location.assign(url); return; }
    if (!drawer.classList.contains("open")) opener = button || document.activeElement;
    swap(document.importNode(next, true));
  } catch {
    form.submit();
  } finally {
    // The product page's form stays; the drawer's were replaced.
    buttons.forEach((b) => { b.disabled = false; });
  }
});

// Apple Pay / Google Pay, once the drawer is open on a cart that can be
// paid for: Stripe.js loads then, not with every page.
let stripe = null;
let stripeElements = null;
let loading = null;
function loadStripe() {
  loading ||= new Promise((resolve, reject) => {
    if (window.Stripe) { resolve(); return; }
    const script = document.createElement("script");
    script.src = "https://js.stripe.com/v3/";
    script.onload = resolve;
    script.onerror = reject;
    document.head.append(script);
  });
  return loading;
}
function payError(message) {
  const error = drawer.querySelector("[data-drawer-pay-error]");
  if (!error) return;
  error.textContent = message;
  error.hidden = false;
}
async function express() {
  const slot = drawer.querySelector("[data-drawer-express]");
  const key = drawer.dataset.publishableKey;
  if (!slot || !key || stripeElements) return;
  try { await loadStripe(); } catch { return; }
  if (stripeElements || !window.Stripe) return;
  stripe = Stripe(key);
  stripeElements = stripe.elements({
    mode: "payment",
    amount: Number(drawer.dataset.amount),
    currency: drawer.dataset.currency,
    appearance: { variables: { colorPrimary: "#e8606a", borderRadius: "6px" } },
  });
  const button = stripeElements.create("expressCheckout", { buttonHeight: 44 });
  button.mount("#drawer-express-checkout");
  button.on("ready", ({ availablePaymentMethods }) => {
    if (availablePaymentMethods) drawer.querySelector("[data-drawer-express]").hidden = false;
  });
  // The shop's shipping methods; none when shipping is included.
  const rates = () => JSON.parse(drawer.dataset.shipping || "[]");
  const ratesTo = (code) => rates()
    .filter((rate) => !code || !rate.countries.length || rate.countries.includes(code))
    .map(({ id, displayName, amount }) => ({ id, displayName, amount }));
  const goods = () => Number(drawer.dataset.amount);
  button.on("click", (event) => event.resolve({
    emailRequired: true,
    shippingAddressRequired: true,
    // A whole address, else wallets hand only its country and postal code.
    billingAddressRequired: true,
    allowedShippingCountries: drawer.dataset.countries.split(","),
    shippingRates: rates().length
      ? ratesTo(null)
      : [{ id: "standard", displayName: "Standard delivery", amount: 0 }],
  }));
  // Only the methods that ship to the wallet's address, priced in.
  button.on("shippingaddresschange", (event) => {
    if (!rates().length) { event.resolve(); return; }
    const here = ratesTo(event.address.country);
    if (!here.length) { event.reject(); return; }
    stripeElements.update({ amount: goods() + here[0].amount });
    event.resolve({ shippingRates: here });
  });
  button.on("shippingratechange", (event) => {
    if (rates().length) stripeElements.update({ amount: goods() + event.shippingRate.amount });
    event.resolve();
  });
  button.on("confirm", async (event) => {
    if (!(await pay(event))) event.paymentFailed({ reason: "fail" });
  });
}

// The wallet's billing address, when it is not the delivery one.
function walletBilling(billing, shipping) {
  const at = billing.address, to = shipping.address || {};
  if (!at?.line1) return null;
  const same = (a, b) => (a || "").trim().toLowerCase() === (b || "").trim().toLowerCase();
  return ["line1", "line2", "postal_code", "city", "country"]
    .every((key) => same(at[key], to[key])) ? null : at;
}

// The wallet's details stand in for the checkout's form. Resolves false when
// the shopper stays on the page.
async function pay(event) {
  const billing = event.billingDetails || {};
  const shipping = event.shippingAddress || {};
  const address = shipping.address || {};
  const rate = event.shippingRate || {};
  const shipped = JSON.parse(drawer.dataset.shipping || "[]").length > 0;
  const body = new URLSearchParams({
    expected_total_cents: String(Number(drawer.dataset.amount) + (shipped ? rate.amount || 0 : 0)),
    shipping_method: shipped ? rate.id || "" : "",
    email: billing.email || "",
    name: shipping.name || billing.name || "",
    phone: billing.phone || "",
    line1: address.line1 || "",
    line2: address.line2 || "",
    postal_code: address.postal_code || "",
    city: address.city || "",
    country: address.country || "",
    note: "",
    vat_number: "",
  });
  // Billed elsewhere when the wallet says so.
  const other = walletBilling(billing, shipping);
  if (other) {
    body.set("billing_different", "1");
    body.set("billing_name", billing.name || shipping.name || "");
    for (const key of ["line1", "line2", "postal_code", "city", "country"]) {
      body.set(`billing_${key}`, other[key] || "");
    }
  }
  try {
    const { error: invalid } = await stripeElements.submit();
    if (invalid) { payError(invalid.message); return false; }
    const response = await fetch(drawer.dataset.startPath, {
      method: "POST",
      body,
      headers: { accept: "application/json" },
    });
    const answer = await response.json();
    if (answer.redirect) { location.assign(answer.redirect); return true; }
    if (answer.error) { payError(answer.error); return false; }
    const { error: refused } = await stripe.confirmPayment({
      elements: stripeElements,
      clientSecret: answer.client_secret,
      confirmParams: { return_url: new URL(drawer.dataset.returnPath, location.href).href },
    });
    payError(refused.message);
    return false;
  } catch {
    payError("Something went wrong. Please try again.");
    return false;
  }
}
})();
"##;
