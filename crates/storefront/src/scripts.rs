//! The scripts the pages run: the cart drawer, the checkout, the menu.
//! Static and trusted; everything they need comes from the `data-*`
//! attributes the headless components render, so they carry no wording and
//! no colour of their own.

use topcoat_core::error::Result;
use topcoat_view::{Unescaped, View};
use topcoat_view_macro::{component, view};

use crate::pages::CheckoutModel;

/// Opens and closes the drawer, sends its forms (and any
/// `[data-add-to-cart]` form on the page) without leaving the page, and
/// offers Apple Pay / Google Pay from it. A form's answer is the page it
/// comes back to, rendered whole: the new drawer is taken from it.
///
/// Reads `[data-cart-drawer]` and its `data-*`, `[data-cart-link]`,
/// `[data-cart-count]`, `[data-drawer-close]`, `[data-drawer-express]`,
/// `#drawer-express-checkout` and `[data-drawer-pay-error]`. The drawer
/// opens on the `open` class and slides out on `shut`.
pub const CART_SCRIPT: &str = r##"
(() => {
let drawer = document.querySelector("[data-cart-drawer]");
if (!drawer) return;
const link = document.querySelector("[data-cart-link]");
let opener = null;
const closer = () => drawer.querySelector("[data-drawer-close]:not([tabindex='-1'])");

function open() {
  if (!drawer.classList.contains("open")) opener = document.activeElement;
  drawer.classList.remove("shut");
  drawer.classList.add("open");
  closer()?.focus({ preventScroll: true });
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

// The drawer of a page rendered by the server replaces this one; the
// wallet buttons, already mounted, move across.
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
  (again || closer())?.focus({ preventScroll: true });
  express();
}

document.addEventListener("submit", async (event) => {
  const form = event.target;
  const inDrawer = drawer.contains(form);
  if (!inDrawer && !form.matches("[data-add-to-cart]")) return;
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
function appearance() {
  const color = drawer.dataset.colorPrimary;
  return color ? { variables: { colorPrimary: color, borderRadius: "6px" } } : {};
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
    appearance: appearance(),
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
      : [{ id: "standard", displayName: drawer.dataset.msgStandardShipping || "Standard shipping", amount: 0 }],
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
    payError(drawer.dataset.msgError || "Something went wrong.");
    return false;
  }
}
})();
"##;

/// Mounts the provider's Payment and Express Checkout elements before any
/// payment is open, keeps the VAT and shipping lines in step with the
/// country and the method, and pays on submit: ask the store to open the
/// payment, then confirm it with the provider.
///
/// Reads `[data-checkout-form]` and its `data-*`, the form's `country`,
/// `vat_number`, `shipping_method`, `billing_different` fields,
/// `[data-billing]`, `[data-no-shipping]`, `[data-checkout-vat]`,
/// `[data-checkout-shipping]`, `[data-checkout-total]`, `[data-place]`,
/// `[data-place-total]`, `[data-payment-error]`, `[data-express]`,
/// `#express-checkout` and `#payment-element`.
pub const CHECKOUT_SCRIPT: &str = r##"
(() => {
const form = document.querySelector("[data-checkout-form]");
if (!form) return;
const button = form.querySelector("[data-place]");
const error = document.querySelector("[data-payment-error]");
const vat = document.querySelector("[data-checkout-vat]");
const country = form.elements.country;
// Absent when the shop does not sell to businesses.
const vatNumber = form.elements.vat_number;
const seller = form.dataset.sellerCountry || "";

// How the shop writes money, as the page did.
const fmt = JSON.parse(form.dataset.money || "{}");
const money = (cents) => {
  const whole = String(Math.floor(cents / 100));
  const grouped = fmt.group ? whole.replace(/\B(?=(\d{3})+(?!\d))/g, fmt.group) : whole;
  const amount = `${grouped}${fmt.decimal || "."}${String(cents % 100).padStart(2, "0")}`;
  const unit = fmt.unit || form.dataset.currency.toUpperCase();
  const gap = fmt.gap ?? " ";
  return fmt.unitFirst ? `${unit}${gap}${amount}` : `${amount}${gap}${unit}`;
};

// The shipping method picked, among those that ship to the country.
const methods = [...form.querySelectorAll("input[name=shipping_method]")];
const noShipping = form.querySelector("[data-no-shipping]");
const shippingLine = document.querySelector("[data-checkout-shipping]");
const totalLine = document.querySelector("[data-checkout-total]");
const placeTotal = form.querySelector("[data-place-total]");
const goods = Number(form.dataset.goods);
const picked = () => methods.find((method) => method.checked && !method.disabled);

function showVat() {
  if (!vat) return;
  const texts = JSON.parse(country.selectedOptions[0]?.dataset.vat || "{}");
  const text = texts[picked()?.value || ""];
  // A business buyer abroad is billed without VAT.
  const reverseCharge = !!vatNumber && vatNumber.value.trim() !== "" && country.value !== seller;
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
  if (totalLine) totalLine.textContent = money(total);
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
  if (!billing || !billingDifferent) return;
  billing.hidden = !billingDifferent.checked;
  billing.disabled = !billingDifferent.checked;
}
billingDifferent?.addEventListener("change", showBilling);

// The wallet's billing address, when it is not the delivery one.
function walletBilling(billing, shipping) {
  const at = billing.address, to = shipping.address || {};
  if (!at?.line1) return null;
  const same = (a, b) => (a || "").trim().toLowerCase() === (b || "").trim().toLowerCase();
  return ["line1", "line2", "postal_code", "city", "country"]
    .every((key) => same(at[key], to[key])) ? null : at;
}

function fail(message) {
  if (error) {
    error.textContent = message;
    error.hidden = false;
  }
  if (button) button.disabled = false;
}

const key = form.dataset.publishableKey;
const color = form.dataset.colorPrimary;
let stripe = null;
let elements = null;
if (key && window.Stripe) {
  stripe = Stripe(key);
  elements = stripe.elements({
    mode: "payment",
    amount: Number(form.dataset.amount),
    currency: form.dataset.currency,
    appearance: color ? { variables: { colorPrimary: color, borderRadius: "6px" } } : {},
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
      : [{ id: "standard", displayName: form.dataset.msgStandardShipping || "Standard shipping", amount: 0 }],
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
    const fill = (name, value) => { if (value && form.elements[name]) form.elements[name].value = value; };
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
    if (billingDifferent) {
      billingDifferent.checked = !!other;
      showBilling();
      if (other) {
        fill("billing_name", billing.name || shipping.name);
        for (const key of ["line1", "line2", "postal_code", "city", "country"]) {
          if (form.elements[`billing_${key}`]) form.elements[`billing_${key}`].value = other[key] || "";
        }
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
  const value = (name) => form.elements[name]?.value.trim() || "";
  const at = billingDifferent?.checked ? "billing_" : "";
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
  if (error) error.hidden = true;
  if (button) button.disabled = true;
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
    fail(form.dataset.msgError || "Something went wrong.");
    return false;
  }
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  pay();
});
})();
"##;

/// Opens a menu panel on hover where there is a mouse, keeps one open at a
/// time, and closes it on Escape or a click elsewhere. Reads every
/// `details[data-menu]` and the `[data-menu-item]` around it; hover applies
/// above the `data-menu-hover-min` width of the first one (901px unless
/// set).
pub const MENU_SCRIPT: &str = r##"
(() => {
const menus = [...document.querySelectorAll("details[data-menu]")];
if (!menus.length) return;
const min = menus[0].dataset.menuHoverMin || "901px";
const hover = matchMedia(`(hover: hover) and (pointer: fine) and (min-width: ${min})`);

for (const menu of menus) {
  const item = menu.closest("[data-menu-item]") || menu;
  const summary = menu.querySelector("summary");
  item.addEventListener("mouseenter", () => { if (hover.matches) menu.open = true; });
  item.addEventListener("mouseleave", () => { if (hover.matches) menu.open = false; });
  // The pointer that opened it on the way in would close it again on click.
  summary.addEventListener("click", (event) => {
    if (hover.matches && event.detail > 0 && menu.open) event.preventDefault();
  });
  menu.addEventListener("toggle", () => {
    if (menu.open) for (const other of menus) if (other !== menu) other.open = false;
  });
}
document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  const menu = menus.find((menu) => menu.open);
  if (!menu) return;
  menu.open = false;
  menu.querySelector("summary").focus();
});
document.addEventListener("click", (event) => {
  for (const menu of menus) if (menu.open && !menu.contains(event.target)) menu.open = false;
});
})();
"##;

/// The cart drawer's script, in a `<script>`; put it after the drawer.
#[component]
pub async fn cart_script() -> Result<impl View> {
    Ok(view! { <script>(Unescaped::new_unchecked(CART_SCRIPT))</script> })
}

/// The checkout's script, in a `<script>`, with the provider's own script
/// before it when the shop takes payments; put it after the form.
#[component]
pub async fn checkout_script(page: &CheckoutModel) -> Result<impl View> {
    Ok(view! {
        if page.setup.is_some() {
            <script src="https://js.stripe.com/v3/"></script>
        }
        <script>(Unescaped::new_unchecked(CHECKOUT_SCRIPT))</script>
    })
}

/// The menu's script, in a `<script>`; put it after the menu.
#[component]
pub async fn menu_script() -> Result<impl View> {
    Ok(view! { <script>(Unescaped::new_unchecked(MENU_SCRIPT))</script> })
}
