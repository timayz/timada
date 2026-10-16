import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { MAILDEV, latestCode, mailCount, registerWithEmail, uniqueEmail } from "./email";
import { FakeStripe, fakeStripeJs, stripeSignature } from "./stripe";
import { FakeVies } from "./vies";
import { createTenant, setLegalIdentity, uniqueTenant } from "./tenants";

// The headless storefront end to end: a merchant publishes a product and
// mints an API key in the dashboard; the example storefront (a separate
// process built on timada-storefront) connects to the store API with the
// key, shows the product, follows the catalog live, browses the categories
// the products are filed under, and (with a key that may sell) fills a cart
// and pays for it with Stripe (a stand-in, see stripe.ts); the merchant then
// fulfils the order or cancels and refunds it.

const STORE_ADDR = process.env.E2E_STORE_ADDR ?? "127.0.0.1:4321";
const STOREFRONT_PORT = Number(process.env.E2E_STOREFRONT_PORT ?? 3456);
const STOREFRONT = `http://127.0.0.1:${STOREFRONT_PORT}`;

let child: ChildProcess | undefined;
let dir: string | undefined;

test.beforeEach(async ({ page }) => {
  await fakeStripeJs(page);
});

test.afterEach(async () => {
  child?.kill();
  child = undefined;
  if (dir) {
    fs.rmSync(dir, { recursive: true, force: true });
    dir = undefined;
  }
});

/// A 1x1 PNG, as the bytes a browser posts when a file is chosen.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

/// Mints a key on the tenant's page and reads it, with the address the
/// tenant's pictures are served from and the dashboard's own. `sell` adds
/// store:checkout to the default read scope.
async function mintKey(
  page: Page,
  slug: string,
  sell = false,
): Promise<{ token: string; mediaUrl: string; apiUrl: string }> {
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Label").fill("Example storefront");
  if (sell) {
    await page.getByRole("checkbox", { name: "store:checkout" }).first().check();
  }
  await page.getByRole("button", { name: "Create key" }).click();
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_/);
  const mediaUrl = (await page.locator("[data-media-url]").textContent())?.trim() ?? "";
  expect(mediaUrl).toContain("/media/");
  const apiUrl = (await page.locator("[data-api-url]").textContent())?.trim() ?? "";
  expect(apiUrl).toMatch(/^https?:\/\//);
  return { token, mediaUrl, apiUrl };
}

/// Boots the example storefront with `token` and waits for it to have synced
/// at least `products` products.
async function startStorefront(
  request: APIRequestContext,
  { token, mediaUrl, apiUrl }: { token: string; mediaUrl: string; apiUrl?: string },
  products: number,
) {
  dir = fs.mkdtempSync(path.join(os.tmpdir(), "timada-storefront-"));
  child = spawn(path.resolve("target/debug/storefront"), [], {
    env: {
      ...process.env,
      STORE_ADDR,
      STORE_TOKEN: token,
      STORE_MEDIA_URL: mediaUrl,
      ...(apiUrl ? { STORE_API_URL: apiUrl } : {}),
      STORE_DB: path.join(dir, "storefront.db"),
      HOST: "127.0.0.1",
      PORT: String(STOREFRONT_PORT),
    },
    stdio: "inherit",
  });
  await expect(async () => {
    const response = await request.get(`${STOREFRONT}/store/status`);
    expect(response.ok()).toBe(true);
    const status = (await response.json()) as { connected: boolean; products: number };
    expect(status.connected).toBe(true);
    expect(status.products).toBeGreaterThanOrEqual(products);
  }).toPass({ timeout: 30_000 });
}

/// Creates a published product priced at `amount`, filed under the category
/// whose menu text contains `category` when one is given.
async function publishProduct(page: Page, slug: string, name: string, amount: string, category?: string) {
  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const url = page.url().split("?")[0];
  await page.getByLabel("Price").fill(amount);
  await page.getByRole("button", { name: "Set price" }).click();
  if (category) {
    await expect(async () => {
      await page.goto(url);
      await expect(page.getByLabel("Category")).toContainText(category, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    const select = page.getByLabel("Category");
    const value = await select.locator("option", { hasText: category }).first().getAttribute("value");
    await select.selectOption(value ?? "");
    await page.getByRole("button", { name: "Add to category" }).click();
    await expect(page.getByRole("status")).toContainText("Added to the category.");
  }
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");
  return url;
}

test("a storefront serves the tenant's published catalog with its key", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Mug Shop");
  await createTenant(page, name);

  // A published product.
  await page.getByRole("link", { name: "Products", exact: true }).click();
  await page.getByLabel("Name").fill("Blue Mug");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  await page.getByLabel("Price").fill("12.50");
  await page.getByRole("button", { name: "Set price" }).click();
  await page.getByLabel("Description").fill("Holds 50 cl of anything hot.");
  await page.getByRole("button", { name: "Save description" }).click();
  await page.getByLabel("Add a picture").setInputFiles({ name: "mug.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill("A blue mug");
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");
  const productUrl = page.url().split("?")[0];

  // A key, shown once; the storefront boots with it and syncs the catalog
  // into its own file.
  const key = await mintKey(page, slug);
  await startStorefront(request, key, 1);

  await page.goto(`${STOREFRONT}/`);
  const card = page.locator("[data-product=blue-mug]");
  await expect(card).toContainText("Blue Mug");
  await expect(card.locator("[data-price]")).toHaveText("12.50 EUR");
  await expect(card.locator("img.thumb")).toHaveAttribute("alt", "A blue mug");
  await card.getByRole("link", { name: "Blue Mug" }).click();
  await expect(page).toHaveURL(`${STOREFRONT}/p/blue-mug`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Blue Mug");
  await expect(page.locator(".description")).toContainText("Holds 50 cl");

  // The picture is loaded from the dashboard's media address, not the
  // storefront's own, and the browser really fetches it.
  const photo = page.locator("[data-gallery] img").first();
  await expect(photo).toHaveAttribute("alt", "A blue mug");
  await expect(photo).toHaveAttribute("src", /timada\.localhost\/media\//);
  await expect(photo).toHaveJSProperty("naturalWidth", 1);
  expect((await page.goto(`${STOREFRONT}/p/nothing-here`))?.status()).toBe(404);

  // Taken off sale in the dashboard, gone from the storefront.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Unpublish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "draft");
  await expect(async () => {
    await page.goto(`${STOREFRONT}/`);
    await expect(page.getByText("Nothing on sale right now.")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
});

test("a storefront browses the categories its products are filed under", async ({ page, request, browser }) => {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Cat Shop");
  await createTenant(page, name);

  // Bedding > Mats, with two mats in it, and Toys beside it with one toy.
  await page.goto(`/t/${slug}/categories`);
  await page.getByLabel("Name").fill("Bedding");
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");
  await expect(async () => {
    await page.goto(`/t/${slug}/categories`);
    await expect(page.locator("[data-category=bedding]")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.getByLabel("Name").fill("Mats");
  const parent = page.getByLabel("Parent");
  await parent.selectOption(
    (await parent.locator("option", { hasText: "Bedding" }).first().getAttribute("value")) ?? "",
  );
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");
  await page.goto(`/t/${slug}/categories`);
  await page.getByLabel("Name").fill("Toys");
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");

  await publishProduct(page, slug, "Cooling Mat", "29.90", "Mats");
  await publishProduct(page, slug, "Wool Mat", "19.90", "Mats");
  await publishProduct(page, slug, "Feather Wand", "9.90", "Toys");

  const key = await mintKey(page, slug);
  await startStorefront(request, key, 3);

  // The menu carries the tree with its counts; Bedding counts its subtree.
  // Bedding opens a panel led by "All Bedding"; Toys has nothing under it.
  const menu = page.locator("nav.sections");
  await expect(async () => {
    await page.goto(`${STOREFRONT}/`);
    await expect(menu.locator("[data-category=bedding]")).toHaveAttribute("data-products", "2", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(menu.locator("[data-category=mats]")).toHaveAttribute("data-products", "2");
  await expect(menu.locator("[data-category=toys]")).toHaveAttribute("data-products", "1");
  await expect(menu.getByRole("link", { name: "All products" })).toBeVisible();
  await expect(menu.locator("[data-category=bedding]")).toBeHidden();
  await menu.getByText("Bedding", { exact: true }).hover();
  await expect(menu.getByRole("link", { name: "All Bedding" })).toBeVisible();
  await expect(menu.locator("[data-category=mats]")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(menu.locator("[data-category=mats]")).toBeHidden();

  // The shop and the back office agree on every one of those numbers.
  await page.goto(`/t/${slug}/categories`);
  for (const [category, count] of [
    ["bedding", "2"],
    ["mats", "2"],
    ["toys", "1"],
  ]) {
    await expect(page.locator(`[data-category=${category}] [data-category-count]`)).toHaveText(count);
  }
  await expect(page.locator("[data-category=bedding] [data-category-direct]")).toHaveAttribute("data-category-direct", "0");
  await page.goto(`${STOREFRONT}/`);

  // A top-level category lists what is under it, newest first by default.
  await menu.getByText("Bedding", { exact: true }).click();
  await page.locator("nav.sections [data-category=bedding]").click();
  await expect(page).toHaveURL(`${STOREFRONT}/c/bedding`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Bedding");
  await expect(page.locator("[data-subcategory=mats]")).toContainText("Mats (2)");
  await expect(page.locator("ul.catalog li")).toHaveCount(2);
  await expect(page.locator("[data-product=feather-wand]")).toHaveCount(0);

  // Sorting is a plain GET form.
  await page.getByLabel("Sort by").selectOption("price_asc");
  await page.getByRole("button", { name: "Sort" }).click();
  await expect(page).toHaveURL(/sort=price_asc/);
  await expect(page.locator("ul.catalog li").first()).toContainText("Wool Mat");
  await page.getByLabel("Sort by").selectOption("price_desc");
  await page.getByRole("button", { name: "Sort" }).click();
  await expect(page.locator("ul.catalog li").first()).toContainText("Cooling Mat");

  // A sub-category page carries a breadcrumb up to its parent.
  await page.goto(`${STOREFRONT}/c/mats`);
  await expect(page.locator("[data-crumbs]")).toContainText("Bedding");
  await expect(page.locator("[data-crumbs]")).toContainText("Mats");
  expect((await page.goto(`${STOREFRONT}/c/nothing-here`))?.status()).toBe(404);

  // A product page carries its category and what else is in it.
  await page.goto(`${STOREFRONT}/p/cooling-mat`);
  await expect(page.locator("[data-crumbs]")).toContainText("Mats");
  const related = page.locator("section.related");
  await expect(related).toContainText("You may also like");
  await expect(related.locator("[data-product=wool-mat]")).toBeVisible();
  await expect(related.locator("[data-product=cooling-mat]")).toHaveCount(0);

  // On a phone without JS, "Menu" opens the whole tree as a sheet.
  const phone = await browser.newContext({ javaScriptEnabled: false, viewport: { width: 390, height: 800 } });
  const small = await phone.newPage();
  await small.goto(`${STOREFRONT}/`);
  await expect(small.locator("nav.sections")).toBeHidden();
  await small.getByRole("link", { name: "Menu", exact: true }).click();
  await expect(small).toHaveURL(/#menu$/);
  await small.locator("nav.sections").getByText("Bedding", { exact: true }).click();
  await small.locator("nav.sections [data-category=mats]").click();
  await expect(small).toHaveURL(`${STOREFRONT}/c/mats`);
  await phone.close();

  // A category the merchant hides leaves the menu, with what is under it;
  // its page still answers.
  await page.goto(`/t/${slug}/categories`);
  await page.locator("[data-category=bedding] a").first().click();
  await page.getByLabel("Show in the shop menu").uncheck();
  await page.getByRole("button", { name: "Save menu visibility" }).click();
  await expect(page.getByRole("status")).toContainText("Hidden from the menu.");
  await expect(async () => {
    await page.goto(`${STOREFRONT}/`);
    await expect(page.locator("nav.sections [data-category=toys]")).toBeVisible({ timeout: 1000 });
    await expect(page.locator("nav.sections [data-menu=bedding]")).toHaveCount(0, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("nav.sections [data-category=mats]")).toHaveCount(0);
  await page.goto(`${STOREFRONT}/c/mats`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Mats");
  await expect(page.locator("[data-crumbs]")).toContainText("Bedding");
});

test("a shopper fills a cart and checks out, refused while the shop takes no payments", async ({ page, request }) => {
  test.setTimeout(60_000);
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Order Shop");
  await createTenant(page, name);
  await publishProduct(page, slug, "Blue Mug", "12.50");
  await publishProduct(page, slug, "Red Plate", "8");

  const key = await mintKey(page, slug, true);
  await startStorefront(request, key, 2);

  // Into the cart, from two product pages.
  await page.goto(`${STOREFRONT}/p/blue-mug`);
  await page.getByLabel("Quantity").fill("2");
  await addToCart(page);
  await expect(page).toHaveURL(`${STOREFRONT}/p/blue-mug`);
  await expect(page.locator("[data-drawer-total]")).toHaveText("25.00 EUR");
  await page.goto(`${STOREFRONT}/p/red-plate`);
  await addToCart(page);
  await expect(page.locator("[data-drawer-total]")).toHaveText("33.00 EUR");
  await page.goto(`${STOREFRONT}/cart`);
  await expect(page.locator("[data-cart-total]")).toHaveText("33.00 EUR");

  // Change a quantity, then take a line out.
  await page.getByLabel("Quantity of Blue Mug").fill("3");
  await page.locator("[data-cart-line='Blue Mug']").getByRole("button", { name: "Update" }).click();
  await expect(page.locator("[data-cart-total]")).toHaveText("45.50 EUR");
  await page.getByRole("button", { name: "Remove Red Plate" }).click();
  await expect(page.locator("[data-cart-line='Red Plate']")).toHaveCount(0);
  await expect(page.locator("[data-cart-total]")).toHaveText("37.50 EUR");

  // An open cart shows in the dashboard.
  await expect(async () => {
    await page.goto(`/t/${slug}/carts`);
    await expect(page.locator("[data-cart-items]").first()).toHaveText("3", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  // Checkout: one page, the cart beside the form. The shop takes no card
  // payments, so there is no card form.
  await page.goto(`${STOREFRONT}/cart`);
  await page.getByRole("link", { name: "Check out" }).click();
  await expect(page.locator("ol.steps [aria-current=step]")).toHaveText("Shipping & payment");
  await expect(page.locator("[data-checkout-total]")).toHaveText("37.50 EUR");
  await expect(page.locator("[data-payments-off]")).toBeVisible();
  await page.getByLabel("E-mail").fill("ada@example.org");
  await page.getByLabel("Full name").fill("Ada Lovelace");
  await page.getByLabel("Street address", { exact: true }).fill("1 rue de la Paix");
  await page.getByLabel("Postal code", { exact: true }).fill("75002");
  await page.getByLabel("City", { exact: true }).fill("Paris");
  await page.getByLabel("Country", { exact: true }).selectOption("FR");

  // The request to pay is refused, the form still filled in: the shop has
  // not said who it is yet, so it could not issue the invoice.
  await page.getByRole("button", { name: "Place order" }).click();
  await expect(page.getByRole("alert")).toContainText("cannot take orders yet", { timeout: 30_000 });
  await expect(page.getByLabel("Full name")).toHaveValue("Ada Lovelace");
  await expect(page.locator("[data-checkout-total]")).toHaveText("37.50 EUR");
});

/// Stripe telling the dashboard a payment went through, as its webhook does.
async function paymentSucceeded(request: APIRequestContext, webhookUrl: string, paymentId: string, secret: string) {
  const body = JSON.stringify({
    id: `evt_${paymentId}`,
    type: "payment_intent.succeeded",
    data: { object: { id: paymentId, object: "payment_intent" } },
  });
  return request.post(webhookUrl, {
    data: body,
    headers: { "content-type": "application/json", "stripe-signature": stripeSignature(body, secret) },
  });
}

/// The checkout's details filled in (or checked kept from the last order).
async function fillCheckout(page: Page, fill: boolean) {
  if (fill) {
    await page.getByLabel("E-mail").fill("ada@example.org");
    await page.getByLabel("Full name").fill("Ada Lovelace");
    await page.getByLabel("Street address", { exact: true }).fill("1 rue de la Paix");
    await page.getByLabel("Postal code", { exact: true }).fill("75002");
    await page.getByLabel("City", { exact: true }).fill("Paris");
    await page.getByLabel("Country", { exact: true }).selectOption("FR");
  } else {
    await expect(page.getByLabel("Full name")).toHaveValue("Ada Lovelace");
  }
}

/// From the cart to Stripe's return page: the checkout filled in (or kept
/// from the last order), the order placed and the payment confirmed (by the
/// fake Stripe.js, see stripe.ts).
/// Adds the product page's pick to the cart: the cart drawer opens on it,
/// without leaving the page.
async function addToCart(page: Page) {
  await page.getByRole("button", { name: "Add to cart" }).click();
  await expect(page.locator("[data-cart-drawer].open .drawer-panel")).toBeVisible();
}

async function placeOrder(page: Page, fill: boolean) {
  await page.goto(`${STOREFRONT}/cart`);
  await page.getByRole("link", { name: "Check out" }).click();
  await fillCheckout(page, fill);
  await page.getByRole("button", { name: "Place order" }).click();
  await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
}

test("a shopper pays with Stripe and the merchant fulfils or refunds the order", async ({ page, request }) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Paying Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    const plateUrl = await publishProduct(page, slug, "Red Plate", "8");

    // The merchant registers its company and the shop sells as it; a SIRET
    // that fails its check is refused on the company's page.
    await page.goto(`/t/${slug}/invoicing`);
    await expect(page.locator("[data-invoicing]")).toHaveAttribute("data-invoicing", "none");
    const company = await setLegalIdentity(page, slug);
    await expect(page.locator("[data-invoicing]")).toHaveAttribute("data-invoicing", "linked");
    await page.goto(company.companyUrl);
    await page.getByLabel("SIRET").fill("12002701600093");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page).toHaveURL(/error=invalid_siret/);
    // VAT-registered, so a product says what kind of goods it is.
    await page.goto(plateUrl);
    await expect(page.locator("[data-product-vat]")).toHaveText("Standard");
    await page.getByLabel("Kind of goods, for VAT").selectOption("food");
    await page.getByRole("button", { name: "Set kind" }).click();
    await expect(page.getByRole("status")).toContainText("Kind of goods set.");
    await expect(page.locator("[data-product-vat]")).toHaveText("Food");

    // The merchant connects a Stripe account; a key Stripe refuses is not saved.
    await page.goto(`/t/${slug}`);
    await page.getByRole("link", { name: "Payments", exact: true }).click();
    await expect(page.locator("[data-payments]")).toHaveAttribute("data-payments", "none");
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    expect(webhookUrl).toMatch(/\/stripe\/webhooks\/[0-9A-Z]{26}$/);
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("not-a-key");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page).toHaveURL(/error=invalid_secret_key/);
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await expect(page.locator("[data-publishable-key]")).toHaveText("pk_test_e2e");

    const key = await mintKey(page, slug, true);
    await startStorefront(request, key, 2);

    // Two mugs. The checkout shows Stripe's form for the total before any
    // payment is open, and the VAT the total includes where the goods go.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await page.getByLabel("Quantity").fill("2");
    await addToCart(page);
    await page.goto(`${STOREFRONT}/cart`);
    await page.getByRole("link", { name: "Check out" }).click();
    const form = page.locator("[data-checkout-form]");
    await expect(form).toHaveAttribute("data-publishable-key", "pk_test_e2e");
    expect(await page.evaluate(() => (window as any).__stripe)).toMatchObject({
      key: "pk_test_e2e",
      elements: { mode: "payment", amount: 2500, currency: "eur" },
      // Asked once, in the form: Stripe's card form asks no country again.
      payment: { fields: { billingDetails: "never" }, wallets: { link: "never" } },
    });
    await expect(page.locator("[data-checkout-total]")).toHaveText("25.00 EUR");
    await expect(page.locator("[data-checkout-vat]")).toHaveText("(incl. 4.17 EUR VAT FR 20%)");
    await page.getByLabel("Country", { exact: true }).selectOption("US");
    await expect(page.locator("[data-checkout-vat]")).toBeHidden();
    await page.getByLabel("Country", { exact: true }).selectOption("FR");
    await expect(page.locator("[data-checkout-vat]")).toBeVisible();

    // The cart changes from the checkout and stays there.
    await page.getByRole("button", { name: "One more Blue Mug" }).click();
    await expect(page).toHaveURL(`${STOREFRONT}/checkout`);
    await expect(page.locator("[data-checkout-total]")).toHaveText("37.50 EUR");
    await page.getByRole("button", { name: "One less Blue Mug" }).click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("25.00 EUR");

    // Placing the order opens the payment and confirms it with Stripe.
    await fillCheckout(page, true);
    await page.getByRole("button", { name: "Place order" }).click();
    await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
    const intent = stripe.lastIntent()!;
    expect(intent.amount).toBe(2500);
    expect(intent.currency).toBe("eur");
    expect(new URL(page.url()).searchParams.get("payment_intent_client_secret")).toBe(`${intent.id}_secret_e2e`);
    expect(JSON.parse((await page.evaluate(() => sessionStorage.getItem("confirmed")))!)).toEqual({
      billing_details: {
        name: "Ada Lovelace",
        email: "ada@example.org",
        phone: "",
        address: { line1: "1 rue de la Paix", line2: "", city: "Paris", postal_code: "75002", state: "", country: "FR" },
      },
    });

    // Stripe sends the browser back before it tells the dashboard: the page
    // waits. A webhook that is not Stripe's is refused.
    await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
    await expect(page.locator("[data-payment-confirming]")).toBeVisible();
    stripe.pay(intent.id);
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_forged")).status()).toBe(400);
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    const reference = (await page.locator("[data-order-reference]").textContent())?.trim() ?? "";
    expect(reference).toMatch(/^[0-9A-Z]{4}-[0-9A-Z]{4}$/);
    await expect(page.locator("[data-order-status]")).toHaveAttribute("data-order-status", "paid");
    await expect(page.locator("[data-order-total]")).toHaveText("25.00 EUR");
    // The dashboard invoices it a moment later; the shopper downloads it.
    const year = new Date().getUTCFullYear();
    const invoiceLink = page.locator(`[data-document="F${year}-000001"]`);
    await expect(async () => {
      await page.reload();
      await expect(invoiceLink).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    const invoicePdf = await page.request.get(new URL((await invoiceLink.getAttribute("href"))!, page.url()).toString());
    expect(invoicePdf.status()).toBe(200);
    expect(invoicePdf.headers()["content-type"]).toBe("application/pdf");
    expect(invoicePdf.headers()["content-disposition"]).toContain(`F${year}-000001.pdf`);
    expect((await invoicePdf.body()).subarray(0, 5).toString()).toBe("%PDF-");
    // Told twice, still one order.
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
    await page.goto(`${STOREFRONT}/cart`);
    await expect(page.locator("[data-cart-empty]")).toBeVisible();

    // The merchant finds it paid and fulfils it.
    const row = page.locator(`[data-order="${reference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(row).toHaveAttribute("data-order-status", "paid", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: reference }).click();
    await expect(page.locator("[data-order-payment]")).toContainText(intent.id);
    await expect(page.locator(`[data-order-documents] [data-document="F${year}-000001"]`)).toBeVisible();
    await expect(page.locator("[data-order-address]")).toContainText("Ada Lovelace");
    await page.getByLabel("Tracking number (optional)").fill("6A12345");
    await page.getByRole("button", { name: "Mark as fulfilled" }).click();
    await expect(page.getByRole("status")).toContainText("Order marked as fulfilled.");
    await expect(page.locator("[data-order-tracking]")).toHaveText("6A12345");
    await expect(page.getByRole("button", { name: "Cancel and refund" })).toHaveCount(0);

    // A second order is cancelled: the money goes back through Stripe.
    await page.goto(`${STOREFRONT}/p/red-plate`);
    await addToCart(page);
    await placeOrder(page, false);
    const second = stripe.lastIntent()!;
    expect(second.id).not.toBe(intent.id);
    stripe.pay(second.id);
    expect((await paymentSucceeded(request, webhookUrl, second.id, "whsec_e2e")).status()).toBe(204);
    await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    const secondReference = (await page.locator("[data-order-reference]").textContent())?.trim() ?? "";
    const secondRow = page.locator(`[data-order="${secondReference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(secondRow).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await secondRow.getByRole("link", { name: secondReference }).click();
    await page.getByLabel("Reason for cancelling").fill("Out of stock");
    await page.getByRole("button", { name: "Cancel and refund" }).click();
    await expect(page.getByRole("status")).toContainText("Order cancelled and refunded.");
    await expect(page.locator("[data-order-refunded]")).toHaveText("8.00 EUR");
    await expect(page.locator("[data-order-cancel-reason]")).toHaveText("Out of stock");
    expect(stripe.refunds).toEqual([{ payment_intent: second.id, amount: 800 }]);

    // The refund is credited against the second invoice.
    const creditNote = page.locator(`[data-order-documents] [data-document="A${year}-000001"]`);
    await expect(async () => {
      await page.reload();
      await expect(creditNote).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(page.locator(`[data-order-documents] [data-document="F${year}-000002"]`)).toBeVisible();
    const creditPdf = await page.request.get((await creditNote.locator("a").getAttribute("href"))!);
    expect(creditPdf.headers()["content-type"]).toBe("application/pdf");

    // The accountant's list holds all three.
    await expect(async () => {
      await page.goto(`/t/${slug}/invoices`);
      await expect(page.locator("[data-document]")).toHaveCount(3, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(page.locator(`[data-document="A${year}-000001"]`)).toContainText("Credit note");
  } finally {
    await stripe.stop();
  }
});

test("the cart drawer: changed in place, with offers, a code and express pay", async ({ page, browser, request }) => {
  test.setTimeout(90_000);
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Drawer Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await publishProduct(page, slug, "Red Plate", "8");
    await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await page.goto(`/t/${slug}/discounts`);
    await page.getByLabel("Code").fill("TEN");
    await page.getByLabel("Percentage or amount").fill("10");
    await page.getByRole("button", { name: "Create discount" }).click();
    await expect(page.getByRole("status")).toContainText("Discount created.");
    await startStorefront(request, await mintKey(page, slug, true), 2);

    // Added from the product page: the drawer slides in, the page stays.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await expect(page.locator("[data-cart-count]")).toBeHidden();
    await addToCart(page);
    await expect(page).toHaveURL(`${STOREFRONT}/p/blue-mug`);
    const drawer = page.locator("[data-cart-drawer]");
    await expect(drawer.locator("[data-summary-line='Blue Mug']")).toBeVisible();
    await expect(page.locator("[data-cart-count]")).toHaveText("1");
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("12.50 EUR");

    // The stepper changes it in place.
    await drawer.getByRole("button", { name: "One more Blue Mug" }).click();
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("25.00 EUR");
    await expect(page.locator("[data-cart-count]")).toHaveText("2");
    await expect(page).toHaveURL(`${STOREFRONT}/p/blue-mug`);

    // What goes with it, added in one click.
    const offers = drawer.locator("[data-upsell]");
    await expect(offers).toContainText("Frequently bought together");
    await expect(offers.locator("[data-offer=blue-mug]")).toHaveCount(0);
    await offers.getByRole("button", { name: "Add Red Plate" }).click();
    await expect(drawer.locator("[data-summary-line='Red Plate']")).toBeVisible();
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("33.00 EUR");
    await expect(drawer.locator("[data-upsell]")).toHaveCount(0);

    // A code, from the drawer.
    await drawer.getByText("Have a promo code?").click();
    await drawer.getByLabel("Discount code").fill("tem");
    await drawer.getByRole("button", { name: "Apply" }).click();
    await expect(drawer.locator("[data-cart-error]")).toContainText("not valid");
    await drawer.getByLabel("Discount code").fill("ten");
    await drawer.getByRole("button", { name: "Apply" }).click();
    await expect(drawer.locator("[data-drawer-discount=TEN]")).toHaveText("-3.30 EUR");
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("29.70 EUR");

    // Closed with Escape, opened again from the header.
    await page.keyboard.press("Escape");
    await expect(drawer.locator(".drawer-panel")).toBeHidden();
    await expect(page.getByRole("button", { name: "Add to cart" })).toBeEnabled();
    await page.locator("[data-cart-link]").click();
    await expect(drawer.locator(".drawer-panel")).toBeVisible();
    await drawer.getByRole("button", { name: "Remove Red Plate" }).click();
    await expect(drawer.locator("[data-summary-line='Red Plate']")).toHaveCount(0);
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("22.50 EUR");

    // Express pay: Stripe's buttons for the drawer's total, paid with what
    // the wallet hands over.
    expect(await page.evaluate(() => (window as any).__stripe)).toMatchObject({
      key: "pk_test_e2e",
      elements: { mode: "payment", amount: 2250, currency: "eur" },
      expressCheckout: { buttonHeight: 44 },
      mounted: ["#drawer-express-checkout"],
    });
    // The wallet is asked for a whole billing address.
    const asked = await page.evaluate(
      () =>
        new Promise((resolve) => (window as any).__stripe.on["expressCheckout:click"]({ resolve })),
    );
    expect(asked).toMatchObject({ emailRequired: true, shippingAddressRequired: true, billingAddressRequired: true });
    // Billed elsewhere than delivered: the order is asked for with both.
    // Not awaited in the page: paying navigates away.
    const started = page.waitForRequest((req) => req.method() === "POST" && req.url().includes("/checkout/start"));
    await page.evaluate(() => {
      void (window as any).__stripe.on["expressCheckout:confirm"]({
        billingDetails: {
          email: "ada@example.com",
          name: "Analytical Engines GmbH",
          address: { line1: "Unter den Linden 1", postal_code: "10117", city: "Berlin", country: "DE" },
        },
        shippingAddress: {
          name: "Ada Lovelace",
          address: { line1: "1 rue de la Paix", postal_code: "75002", city: "Paris", country: "FR" },
        },
        paymentFailed() {},
      });
    });
    const sent = new URLSearchParams((await started).postData() ?? "");
    expect(Object.fromEntries(sent)).toMatchObject({
      city: "Paris",
      billing_different: "1",
      billing_name: "Analytical Engines GmbH",
      billing_line1: "Unter den Linden 1",
      billing_city: "Berlin",
      billing_country: "DE",
    });
    await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
    expect(stripe.lastIntent()!.amount).toBe(2250);

    // Without JavaScript the forms come back to the page, drawer open.
    const plain = await browser.newContext({ javaScriptEnabled: false });
    try {
      const noScript = await plain.newPage();
      await noScript.goto(`${STOREFRONT}/p/red-plate`);
      await noScript.getByRole("button", { name: "Add to cart" }).click();
      await expect(noScript).toHaveURL(`${STOREFRONT}/p/red-plate#cart`);
      const panel = noScript.locator("[data-cart-drawer] .drawer-panel");
      await expect(panel).toBeVisible();
      await expect(panel.locator("[data-drawer-total]")).toHaveText("8.00 EUR");
      await panel.getByRole("button", { name: "One more Red Plate" }).click();
      await expect(noScript).toHaveURL(`${STOREFRONT}/p/red-plate#cart`);
      await expect(panel.locator("[data-drawer-total]")).toHaveText("16.00 EUR");
      await panel.getByRole("button", { name: "Remove Red Plate" }).click();
      await expect(panel.locator("[data-cart-empty]")).toBeVisible();
    } finally {
      await plain.close();
    }
  } finally {
    await stripe.stop();
  }
});

test("a shopper pays less with a discount code, once", async ({ page, request }) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Promo Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");

    // 10 % off, once per shopper.
    await page.goto(`/t/${slug}/discounts`);
    await page.getByLabel("Code").fill("TEN");
    await page.getByLabel("Percentage or amount").fill("10");
    await page.getByLabel("Once per shopper e-mail").check();
    await page.getByRole("button", { name: "Create discount" }).click();
    await expect(page.getByRole("status")).toContainText("Discount created.");
    await startStorefront(request, await mintKey(page, slug, true), 1);

    // A typo is refused at once; the right code takes 2.50 off two mugs.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await page.getByLabel("Quantity").fill("2");
    await addToCart(page);
    await page.goto(`${STOREFRONT}/cart`);
    await page.getByLabel("Discount code").fill("TEM");
    await page.getByRole("button", { name: "Apply" }).click();
    await expect(page.getByRole("alert").first()).toContainText("not valid");
    await page.getByLabel("Discount code").fill("ten");
    await page.getByRole("button", { name: "Apply" }).click();
    await expect(page.locator("[data-cart-subtotal]")).toHaveText("25.00 EUR");
    await expect(page.locator("[data-cart-discount=TEN] [data-discount-amount]")).toHaveText("-2.50 EUR");
    await expect(page.locator("[data-cart-total]")).toHaveText("22.50 EUR");

    // Charged the discounted total, and the order and its invoice say so.
    await page.getByRole("link", { name: "Check out" }).click();
    await expect(page.locator("[data-checkout-discount=TEN]")).toHaveText("-2.50 EUR");
    await expect(page.locator("[data-checkout-total]")).toHaveText("22.50 EUR");
    await placeOrder(page, true);
    const intent = stripe.lastIntent()!;
    expect(intent.amount).toBe(2250);
    stripe.pay(intent.id);
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
    await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    await expect(page.locator("[data-order-discount=TEN]")).toContainText("2.50 EUR");
    await expect(page.locator("[data-order-total]")).toHaveText("22.50 EUR");
    const year = new Date().getUTCFullYear();
    const invoiceLink = page.locator(`[data-document="F${year}-000001"]`);
    await expect(async () => {
      await page.reload();
      await expect(invoiceLink).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(page.locator("[data-documents] li", { has: invoiceLink })).toContainText("22,50");
    const reference = (await page.locator("[data-order-reference]").textContent())?.trim() ?? "";

    // The same shopper cannot use it twice: back to the cart, told why.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await addToCart(page);
    await page.goto(`${STOREFRONT}/cart`);
    await page.getByRole("link", { name: "Check out" }).click();
    // Entered from the checkout's summary this time.
    await page.getByText("Have a promo code?").click();
    await page.getByLabel("Discount code").fill("TEN");
    await page.getByRole("button", { name: "Apply" }).click();
    await expect(page).toHaveURL(`${STOREFRONT}/checkout`);
    await expect(page.locator("[data-checkout-discount=TEN]")).toHaveText("-1.25 EUR");
    await page.getByRole("button", { name: "Place order" }).click();
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/cart\\?error=discount_already_used`), { timeout: 30_000 });
    await expect(page.getByRole("alert").first()).toContainText("already used this discount code");

    // The merchant sees the discount on the order and the use on the code.
    const row = page.locator(`[data-order="${reference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: reference }).click();
    await expect(page.locator("[data-order-discount=TEN]")).toContainText("-2.50 EUR");
    await page.goto(`/t/${slug}/discounts`);
    await expect(page.locator("[data-discount=TEN] [data-discount-orders]")).toHaveText("1");
  } finally {
    await stripe.stop();
  }
});

test("a shopper picks a delivery method that ships to their country, and pays for it", async ({ page, request }) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Ship Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");

    // Tracked to France and Belgium, free from 50.00; VIP to France only.
    await page.goto(`/t/${slug}/shipping`);
    await page.getByLabel("Name", { exact: true }).fill("Tracked delivery");
    await page.getByLabel("Price").fill("4.90");
    await page.getByLabel("Free from (optional)").fill("50");
    await page.getByRole("checkbox", { name: "France FR" }).check();
    await page.getByRole("checkbox", { name: "Belgium BE" }).check();
    await page.getByRole("button", { name: "Add shipping method" }).click();
    await expect(page.getByRole("status")).toContainText("Shipping method added.");
    await page.getByLabel("Name", { exact: true }).fill("VIP");
    await page.getByLabel("Price").fill("6.90");
    await page.getByRole("checkbox", { name: "France FR" }).check();
    await page.getByRole("button", { name: "Add shipping method" }).click();
    await expect(page.getByRole("status")).toContainText("Shipping method added.");
    await startStorefront(request, await mintKey(page, slug, true), 1);

    // Two mugs: the first method is picked, its price on top.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await page.getByLabel("Quantity").fill("2");
    await addToCart(page);
    await page.goto(`${STOREFRONT}/cart`);
    await page.getByRole("link", { name: "Check out" }).click();
    const tracked = page.getByRole("radio", { name: /Tracked delivery/ });
    const vip = page.getByRole("radio", { name: /VIP/ });
    await expect(tracked).toBeChecked();
    await expect(page.locator("[data-checkout-shipping]")).toHaveText("4.90 EUR");
    await expect(page.locator("[data-checkout-total]")).toHaveText("29.90 EUR");
    await expect(page.locator("[data-checkout-vat]")).toContainText("VAT FR 20%");
    expect(await page.evaluate(() => (window as any).__stripe.elements.amount)).toBe(2990);

    // VIP costs more; Belgium has only the tracked parcel; the US nothing.
    await vip.check();
    await expect(page.locator("[data-checkout-shipping]")).toHaveText("6.90 EUR");
    await expect(page.locator("[data-checkout-total]")).toHaveText("31.90 EUR");
    expect(await page.evaluate(() => (window as any).__stripe.elements.amount)).toBe(3190);
    await page.getByLabel("Country", { exact: true }).selectOption("BE");
    await expect(vip).toBeHidden();
    await expect(tracked).toBeChecked();
    await expect(page.locator("[data-checkout-total]")).toHaveText("29.90 EUR");
    await page.getByLabel("Country", { exact: true }).selectOption("US");
    await expect(tracked).toBeHidden();
    await expect(page.locator("[data-no-shipping]")).toBeVisible();
    await expect(page.locator("[data-checkout-total]")).toHaveText("25.00 EUR");

    // Back to France with VIP: charged on top, on the order and its invoice.
    await fillCheckout(page, true);
    await vip.check();
    await expect(page.locator("[data-checkout-total]")).toHaveText("31.90 EUR");
    await page.getByRole("button", { name: "Place order" }).click();
    await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
    const intent = stripe.lastIntent()!;
    expect(intent.amount).toBe(3190);
    stripe.pay(intent.id);
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
    await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    await expect(page.locator("[data-order-shipping=VIP]")).toContainText("6.90 EUR");
    await expect(page.locator("[data-order-total]")).toHaveText("31.90 EUR");
    const year = new Date().getUTCFullYear();
    const invoiceLink = page.locator(`[data-document="F${year}-000001"]`);
    await expect(async () => {
      await page.reload();
      await expect(invoiceLink).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(page.locator("[data-documents] li", { has: invoiceLink })).toContainText("31,90");
    const reference = (await page.locator("[data-order-reference]").textContent())?.trim() ?? "";

    // The merchant sees what shipping was paid.
    const row = page.locator(`[data-order="${reference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: reference }).click();
    await expect(page.locator("[data-order-shipping=VIP]")).toContainText("6.90 EUR");
  } finally {
    await stripe.stop();
  }
});

/// A review as a storefront hands it in: multipart, with the key.
function reviewForm(orderId: string, productId: string, body: string, photos: Buffer[]): FormData {
  const form = new FormData();
  form.append("order_id", orderId);
  form.append("product_id", productId);
  form.append("rating", "4");
  form.append("title", "Lovely mug");
  form.append("body", body);
  for (const [n, photo] of photos.entries()) {
    form.append("photo", new Blob([photo]), `photo-${n}.png`);
  }
  return form;
}

test("a buyer's review waits for the merchant, who publishes, trims and takes it down", async ({ page, request }) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Reviewed Shop");
    await createTenant(page, name);
    const mugUrl = await publishProduct(page, slug, "Blue Mug", "12.50");
    const productId = mugUrl.split("/").pop() ?? "";
    await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    const reader = await mintKey(page, slug);
    const seller = await mintKey(page, slug, true);
    await startStorefront(request, seller, 1);

    // One mug, paid for.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await addToCart(page);
    await placeOrder(page, true);
    const intent = stripe.lastIntent()!;
    stripe.pay(intent.id);
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
    await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    const orderId = page.url().split("/").pop() ?? "";

    // The storefront hands the review in with a key that may sell.
    const submit = (token: string | null, form: FormData) =>
      request.post("/store/reviews", {
        headers: token ? { authorization: `Bearer ${token}` } : {},
        multipart: form,
      });
    const text = "Holds a big tea and keeps it warm.";
    expect((await submit(null, reviewForm(orderId, productId, text, []))).status()).toBe(401);
    expect((await submit("tk_unknown", reviewForm(orderId, productId, text, []))).status()).toBe(401);
    expect((await submit(reader.token, reviewForm(orderId, productId, text, []))).status()).toBe(403);
    const refusal = async (form: FormData) => {
      const response = await submit(seller.token, form);
      expect(response.status()).toBe(422);
      return ((await response.json()) as { error: string }).error;
    };
    expect(await refusal(reviewForm(orderId, productId, "Too short", []))).toBe("review_invalid_body");
    expect(await refusal(reviewForm(orderId, productId, text, [Buffer.from("not a picture")]))).toBe(
      "review_invalid_photo",
    );
    expect(await refusal(reviewForm(orderId, "not-bought", text, []))).toBe("review_not_purchased");
    expect(await refusal(reviewForm("0".repeat(64), productId, text, []))).toBe("review_order_not_found");
    const created = await submit(seller.token, reviewForm(orderId, productId, text, [PNG, PNG]));
    expect(created.status()).toBe(201);
    const reviewId = ((await created.json()) as { id: string }).id;
    expect(await refusal(reviewForm(orderId, productId, text, []))).toBe("review_already_reviewed");

    // While it waits, the storefront may hand in a change, with the same key.
    const revise = (token: string | null, form: FormData) =>
      request.post("/store/reviews/revise", {
        headers: token ? { authorization: `Bearer ${token}` } : {},
        multipart: form,
      });
    expect((await revise(null, reviewForm(orderId, productId, text, []))).status()).toBe(401);
    expect((await revise(reader.token, reviewForm(orderId, productId, text, []))).status()).toBe(403);
    const unknownPhoto = reviewForm(orderId, productId, text, []);
    unknownPhoto.append("keep_photo", "not-a-photo");
    const unknownRefused = await revise(seller.token, unknownPhoto);
    expect(unknownRefused.status()).toBe(422);
    expect(((await unknownRefused.json()) as { error: string }).error).toBe("review_photo_not_found");

    // The merchant finds it waiting, signed with the name it was shipped to.
    const row = page.locator(`[data-review="${reviewId}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/reviews`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(row).toHaveAttribute("data-review-status", "pending");
    await expect(row).toContainText("Ada L.");
    await row.getByRole("link", { name: "Lovely mug" }).click();
    await expect(page.locator("[data-review-status]")).toHaveAttribute("data-review-status", "pending");
    await expect(page.locator("[data-review-body]")).toHaveText(text);
    const photos = page.locator("[data-review-photo]");
    await expect(photos).toHaveCount(2);
    const photoUrl = async (n: number) => (await photos.nth(n).locator("img").getAttribute("src")) ?? "";
    const first = await photoUrl(0);
    const second = await photoUrl(1);
    expect((await request.get(first)).status()).toBe(200);

    // One photo off; its file goes with it.
    await photos.first().getByRole("button", { name: "Remove this photo" }).click();
    await expect(page.getByRole("status")).toContainText("Photo removed.");
    await expect(photos).toHaveCount(1);
    await expect(async () => expect((await request.get(first)).status()).toBe(404)).toPass({ timeout: 15_000 });

    // Published, it counts in the product's rating.
    await page.goto(mugUrl);
    await expect(page.locator("[data-product-rating]")).toContainText("No published review");
    await page.goto(`/t/${slug}/reviews`);
    await page.getByLabel("Status").selectOption("all");
    await page.getByRole("button", { name: "Filter" }).click();
    await row.getByRole("link", { name: "Lovely mug" }).click();
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.getByRole("status")).toContainText("Review published.");
    await expect(page.locator("[data-review-status]")).toHaveAttribute("data-review-status", "published");
    // Published, it can no longer be changed.
    const late = await revise(seller.token, reviewForm(orderId, productId, "Changed my mind about it.", []));
    expect(late.status()).toBe(422);
    expect(((await late.json()) as { error: string }).error).toBe("review_not_pending");
    await expect(async () => {
      await page.goto(mugUrl);
      await expect(page.locator("[data-product-rating]")).toContainText("★ 4.0 · 1 published", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    // The next shopper reads it beside the checkout.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await addToCart(page);
    await expect(async () => {
      await page.goto(`${STOREFRONT}/checkout`);
      await expect(page.locator("[data-checkout-reviews]")).toContainText("Lovely mug", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await page.goto(mugUrl);

    // Taken down for good, with its last photo.
    await page.getByRole("link", { name: "See its reviews" }).click();
    await row.getByRole("link", { name: "Lovely mug" }).click();
    await page.getByLabel("Why it is rejected").fill("Not about the mug");
    await page.getByRole("button", { name: "Take down" }).click();
    await expect(page.getByRole("status")).toContainText("Review rejected.");
    await expect(page.locator("[data-review-reason]")).toHaveText("Not about the mug");
    await expect(page.getByRole("button", { name: "Publish" })).toHaveCount(0);
    await expect(async () => expect((await request.get(second)).status()).toBe(404)).toPass({ timeout: 15_000 });
    await expect(async () => {
      await page.goto(mugUrl);
      await expect(page.locator("[data-product-rating]")).toContainText("No published review", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
  } finally {
    await stripe.stop();
  }
});

test("a shopper reviews what they bought from the order page, shown once the shop publishes it", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Rated Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await publishProduct(page, slug, "Red Plate", "8");
    await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await startStorefront(request, await mintKey(page, slug, true), 2);

    // A mug and a plate, paid for.
    for (const product of ["blue-mug", "red-plate"]) {
      await page.goto(`${STOREFRONT}/p/${product}`);
      await addToCart(page);
    }
    await placeOrder(page, true);
    const intent = stripe.lastIntent()!;
    stripe.pay(intent.id);
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
    await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    const orderUrl = page.url();

    // Nothing reviewed yet: a form under each product of the order.
    const reviews = page.locator("[data-review-of]");
    await expect(reviews).toHaveCount(2);
    const mug = reviews.filter({ hasText: "Blue Mug" });
    const plate = reviews.filter({ hasText: "Red Plate" });

    // Too short is refused, with what was wrong.
    await plate.getByRole("radio", { name: "2 stars" }).check();
    await plate.getByLabel("Your review").fill("Meh.");
    await plate.getByRole("button", { name: "Send my review" }).click();
    await expect(plate.getByRole("alert")).toContainText("Say a little more");

    // The mug: four stars, a title, a text and a photo.
    await mug.getByRole("radio", { name: "4 stars" }).check();
    await mug.getByLabel("Title (optional)").fill("Lovely mug");
    await mug.getByLabel("Your review").fill("Holds a big tea and keeps it warm.");
    await mug.getByLabel(/^Photos/).setInputFiles({ name: "mug.png", mimeType: "image/png", buffer: PNG });
    await mug.getByRole("button", { name: "Send my review" }).click();
    await expect(mug.getByRole("status")).toContainText("Your review is with the shop.");
    await expect(mug.locator("[data-review-status]")).toHaveAttribute("data-review-status", "pending");
    await expect(mug.getByRole("button", { name: "Send my review" })).toHaveCount(0);

    // Still waiting, the shopper changes it: other words, and another photo.
    const kept = mug.locator("[data-review-photo-keep]");
    await expect(kept).toHaveCount(1);
    const firstPhoto = (await kept.locator("img").getAttribute("src")) ?? "";
    expect((await request.get(firstPhoto)).status()).toBe(200);
    await mug.getByText("Edit my review").click();
    await mug.getByRole("radio", { name: "5 stars" }).check();
    await mug.locator("[data-review-edit]").getByLabel("Your review").fill("Holds a big tea and keeps it hot.");
    await kept.getByRole("checkbox", { name: "Keep this photo" }).uncheck();
    await mug.getByLabel(/^Add photos/).setInputFiles({ name: "mug-2.png", mimeType: "image/png", buffer: PNG });
    await mug.getByRole("button", { name: "Save my changes" }).click();
    await expect(mug.getByRole("status")).toContainText("Your changes are with the shop.");
    await expect(mug.locator("[data-review-status]")).toHaveAttribute("data-review-status", "pending");
    await expect(mug.locator("[data-review-status] .rating")).toHaveAttribute("aria-label", "5 out of 5");
    await expect(kept).toHaveCount(1);
    const secondPhoto = (await kept.locator("img").getAttribute("src")) ?? "";
    expect(secondPhoto).not.toBe(firstPhoto);
    await expect(async () => expect((await request.get(firstPhoto)).status()).toBe(404)).toPass({ timeout: 15_000 });
    expect((await request.get(secondPhoto)).status()).toBe(200);

    // Waiting for the merchant, nothing shows on the shop.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await expect(page.locator("[data-reviews]")).toHaveAttribute("data-reviews", "0");
    await expect(page.locator("[data-product-rating]")).toHaveCount(0);

    // The merchant publishes it.
    const row = page.locator("[data-review-status=pending]", { hasText: "Lovely mug" });
    await expect(async () => {
      await page.goto(`/t/${slug}/reviews`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: "Lovely mug" }).click();
    await expect(page.locator("[data-review-photo]")).toHaveCount(1);
    await expect(page.locator("[data-review-body]")).toHaveText("Holds a big tea and keeps it hot.");
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.getByRole("status")).toContainText("Review published.");
    const reviewPage = page.url();

    // The merchant answers in public, then rewrites the answer.
    await page.getByLabel("Reply", { exact: true }).fill("Thank you!");
    await page.getByRole("button", { name: "Post the reply" }).click();
    await expect(page.getByRole("status")).toContainText("Reply saved.");
    await expect(page.locator("[data-review-reply]")).toHaveText("Thank you!");
    await page.getByLabel("Reply", { exact: true }).fill("Thank you, Ada! Enjoy your tea.");
    await page.getByRole("button", { name: "Update the reply" }).click();
    await expect(page.locator("[data-review-reply]")).toHaveText("Thank you, Ada! Enjoy your tea.");

    // On the product page with its photo, and stars on the card.
    await expect(async () => {
      await page.goto(`${STOREFRONT}/p/blue-mug`);
      await expect(page.locator("[data-product-rating]")).toHaveAttribute("data-product-rating", "5.0", {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });
    const review = page.locator("[data-review]");
    await expect(review).toHaveCount(1);
    await expect(review).toContainText("Lovely mug");
    await expect(review).toContainText("Holds a big tea and keeps it hot.");
    await expect(review).toContainText("Ada L. · Verified purchase");
    await expect(async () => {
      await page.goto(`${STOREFRONT}/p/blue-mug`);
      await expect(review.locator("[data-review-reply]")).toContainText("Thank you, Ada! Enjoy your tea.", {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });
    const photo = (await review.locator("[data-review-photo]").getAttribute("src")) ?? "";
    expect((await request.get(photo)).status()).toBe(200);
    await page.goto(STOREFRONT);
    await expect(page.locator("[data-product=blue-mug] [data-rating]")).toHaveAttribute("data-rating", "5.0");
    await expect(page.locator("[data-product=red-plate] [data-rating]")).toHaveCount(0);

    // The shopper's order page says it is out, and it can no longer be changed.
    await page.goto(orderUrl);
    await expect(mug.locator("[data-review-status]")).toHaveAttribute("data-review-status", "published");
    await expect(mug.locator("[data-review-edit]")).toHaveCount(0);
    await expect(mug.locator("[data-review-reply]")).toContainText("Thank you, Ada! Enjoy your tea.");

    // The answer taken off leaves the shop too.
    await page.goto(reviewPage);
    await page.getByRole("button", { name: "Delete the reply" }).click();
    await expect(page.getByRole("status")).toContainText("Reply deleted.");
    await expect(page.locator("[data-review-reply]")).toHaveCount(0);
    await expect(async () => {
      await page.goto(`${STOREFRONT}/p/blue-mug`);
      await expect(page.locator("[data-review]")).toHaveCount(1, { timeout: 1000 });
      await expect(page.locator("[data-review-reply]")).toHaveCount(0, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
  } finally {
    await stripe.stop();
  }
});

/// From the cart to Stripe's return page, shipping to `country`, as a
/// business when a VAT number is given.
async function checkoutTo(page: Page, country: string, postalCode: string, vatNumber = "") {
  await page.goto(`${STOREFRONT}/cart`);
  await page.getByRole("link", { name: "Check out" }).click();
  await expect(page.locator("[data-import-notice]")).toBeVisible();
  await page.getByLabel("E-mail").fill("ada@example.org");
  await page.getByLabel("Full name").fill("Ada Lovelace");
  await page.getByLabel("Street address", { exact: true }).fill("1 Main Street");
  await page.getByLabel("Postal code", { exact: true }).fill(postalCode);
  await page.getByLabel("City", { exact: true }).fill("Somewhere");
  await page.getByLabel("Country", { exact: true }).selectOption(country);
  // Only a shop that sells to businesses asks for the number.
  const vat = page.getByLabel("EU company VAT number (optional, for an invoice without VAT)");
  if (await vat.count()) await vat.fill(vatNumber);
  await page.getByRole("button", { name: "Place order" }).click();
  await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
}

test("a sale abroad is invoiced at the VAT of where the goods go", async ({ page, request }) => {
  const stripe = new FakeStripe();
  const vies = new FakeVies();
  await stripe.start();
  await vies.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Export Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    const company = await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await startStorefront(request, await mintKey(page, slug, true), 1);

    // Each order: a mug, paid, then told to the dashboard.
    const addMug = async () => {
      await page.goto(`${STOREFRONT}/p/blue-mug`);
      await addToCart(page);
    };
    const pay = async () => {
      const intent = stripe.lastIntent()!;
      expect(intent.amount).toBe(1250);
      stripe.pay(intent.id);
      expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
      await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
      await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    };
    const buy = async (country: string, postalCode: string, vatNumber = "") => {
      await addMug();
      await checkoutTo(page, country, postalCode, vatNumber);
      await pay();
    };
    const year = new Date().getUTCFullYear();
    const regimeOf = (number: number) =>
      page.locator(`[data-document="F${year}-${String(number).padStart(6, "0")}"] [data-document-regime]`);
    const expectRegime = async (number: number, regime: string) => {
      await expect(async () => {
        await page.goto(`/t/${slug}/invoices`);
        await expect(regimeOf(number)).toHaveAttribute("data-document-regime", regime, { timeout: 1000 });
      }).toPass({ timeout: 20_000 });
    };

    // A shop sells to consumers only until its merchant says otherwise: no
    // VAT number is asked for.
    await addMug();
    await page.goto(`${STOREFRONT}/cart`);
    await page.getByRole("link", { name: "Check out" }).click();
    await expect(page.locator("#checkout-vat")).toHaveCount(0);
    await page.goto(`/t/${slug}/invoicing`);
    await expect(page.locator("[data-business-buyers]")).toHaveAttribute("data-business-buyers", "off");
    await page.getByRole("button", { name: "Accept business buyers" }).click();
    await expect(page.getByRole("status")).toContainText("Business buyers accepted.");
    await expect(page.locator("[data-business-buyers]")).toHaveAttribute("data-business-buyers", "on");

    // A German business VIES knows: reverse charge. A bad number is refused
    // at checkout.
    await page.goto(`${STOREFRONT}/cart`);
    await page.getByRole("link", { name: "Check out" }).click();
    await page.getByLabel("E-mail").fill("ada@example.org");
    await page.getByLabel("Full name").fill("Ada Lovelace");
    await page.getByLabel("Street address", { exact: true }).fill("1 Main Street");
    await page.getByLabel("Postal code", { exact: true }).fill("10115");
    await page.getByLabel("City", { exact: true }).fill("Berlin");
    await page.getByLabel("Country", { exact: true }).selectOption("DE");
    await page.getByLabel("EU company VAT number (optional, for an invoice without VAT)").fill("US123");
    await page.getByRole("button", { name: "Place order" }).click();
    await expect(page.getByRole("alert")).toContainText("EU VAT number");
    await checkoutTo(page, "DE", "10115", "DE123456789");
    await pay();
    await expectRegime(1, "reverse_charge");
    expect(vies.checked).toContain("DE123456789");

    // Out of the EU: an export.
    await buy("US", "10001");
    await expectRegime(2, "export");

    // Under the threshold a German consumer pays French VAT; once the
    // merchant joins the One-Stop Shop, German VAT, on the quarter's return.
    await buy("DE", "10115");
    await expectRegime(3, "distance_origin");
    await page.goto(`${company.companyUrl}/oss`);
    await expect(page.locator("[data-oss-empty]")).toBeVisible();
    await page.goto(company.companyUrl);
    await page.getByRole("checkbox", { name: /One-Stop Shop/ }).check();
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Legal identity saved.");
    await buy("DE", "10115");
    await expectRegime(4, "oss");
    await expect(async () => {
      await page.goto(`${company.companyUrl}/oss`);
      await expect(page.locator(`[data-oss-row="DE-1900"] [data-oss-vat]`)).toHaveText("2,00\u00a0€", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await page.goto(company.companyUrl);
    await expect(page.locator("[data-eu-distance]")).toContainText("20,92");

    // Shipped to New York, billed to a company in Berlin: the billing
    // fields show once asked for, Stripe and the invoice get the billing
    // address, and VAT still follows the goods.
    await addMug();
    await page.goto(`${STOREFRONT}/checkout`);
    const billing = page.locator("[data-billing]");
    await expect(billing).toBeHidden();
    await page.getByLabel("E-mail").fill("ada@example.org");
    await page.getByLabel("Full name").fill("Ada Lovelace");
    await page.getByLabel("Street address", { exact: true }).fill("350 Fifth Avenue");
    await page.getByLabel("Postal code", { exact: true }).fill("10118");
    await page.getByLabel("City", { exact: true }).fill("New York");
    await page.getByLabel("Country", { exact: true }).selectOption("US");
    await page.getByLabel("Use a different billing address (optional)").check();
    await expect(billing).toBeVisible();
    await page.getByLabel("Billing name or company").fill("Analytical Engines GmbH");
    await page.getByLabel("Billing street address").fill("Unter den Linden 1");
    await page.getByLabel("Billing city").fill("Berlin");
    await page.getByLabel("Billing postal code").fill("10117");
    await page.getByLabel("Billing country").selectOption("DE");
    await page.getByRole("button", { name: "Place order" }).click();
    await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
    expect(JSON.parse((await page.evaluate(() => sessionStorage.getItem("confirmed")))!).billing_details).toMatchObject({
      name: "Analytical Engines GmbH",
      email: "ada@example.org",
      address: { line1: "Unter den Linden 1", city: "Berlin", postal_code: "10117", country: "DE" },
    });
    await pay();
    await expect(page.locator("[data-order-billing]")).toContainText("Analytical Engines GmbH");
    await expect(page.locator("[data-order-address]")).toContainText("350 Fifth Avenue");
    await expectRegime(5, "export");
    const billed = page.locator(`[data-document="F${year}-000005"]`);
    await expect(billed).toContainText("Analytical Engines GmbH");
    await billed.locator("a").nth(1).click();
    await expect(page.locator("[data-order-billing]")).toContainText("Unter den Linden 1");
    await expect(page.locator("[data-order-address]")).toContainText("New York");

    // Express Checkout: the wallet's billing address, when it is not the
    // delivery one, fills the billing fields and goes to Stripe.
    await addMug();
    await page.goto(`${STOREFRONT}/checkout`);
    await page.getByLabel("Use a different billing address (optional)").uncheck();
    await page.evaluate(() => {
      void (window as any).__stripe.on["expressCheckout:confirm"]({
        billingDetails: {
          email: "ada@example.org",
          name: "Analytical Engines GmbH",
          address: { line1: "Unter den Linden 1", postal_code: "10117", city: "Berlin", country: "DE" },
        },
        shippingAddress: {
          name: "Ada Lovelace",
          address: { line1: "350 Fifth Avenue", postal_code: "10118", city: "New York", country: "US" },
        },
        paymentFailed() {},
      });
    });
    await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
    expect(JSON.parse((await page.evaluate(() => sessionStorage.getItem("confirmed")))!).billing_details).toMatchObject({
      name: "Analytical Engines GmbH",
      email: "ada@example.org",
      address: { line1: "Unter den Linden 1", city: "Berlin", postal_code: "10117", country: "DE" },
    });
  } finally {
    await vies.stop();
    await stripe.stop();
  }
});

/// Signs in on the storefront from its sign-in page (already open): the
/// address, then the code the shop mailed.
async function signInToShop(page: Page, email: string, firstCode?: string) {
  const before = await mailCount(page.request, email);
  await page.getByLabel("E-mail").fill(email);
  await page.getByRole("button", { name: "Send me a code" }).click();
  await expect(page).toHaveURL(/\/account\/sign-in\/code/);
  const code = await latestCode(page.request, email, before);
  if (firstCode) {
    await page.getByLabel("Code", { exact: true }).fill(firstCode);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("This is not the code we sent");
  }
  await page.getByLabel("Code", { exact: true }).fill(code);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
}

test("a returning shopper signs in with a mailed code and finds their orders and address", async ({
  page,
  request,
  browser,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Account Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await publishProduct(page, slug, "Red Plate", "8");
    await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await startStorefront(request, await mintKey(page, slug, true), 2);
    // The confirmations link to the orders on the storefront.
    await page.goto(`/t/${slug}/api-keys`);
    await page.getByLabel("Where shoppers find the shop").fill(STOREFRONT);
    await page.getByRole("button", { name: "Save address" }).click();
    await expect(page.getByRole("status")).toContainText("Storefront address saved.");
    await expect(page.getByLabel("Where shoppers find the shop")).toHaveValue(STOREFRONT);
    const shopper = uniqueEmail("shopper");

    const pay = async (shop: Page) => {
      await expect(shop).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
      const intent = stripe.lastIntent()!;
      stripe.pay(intent.id);
      expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
      await shop.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
      await expect(shop).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
      return shop.url();
    };

    // A guest order first, as anyone.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await addToCart(page);
    await page.goto(`${STOREFRONT}/checkout`);
    await page.getByLabel("E-mail").fill(shopper);
    await page.getByLabel("Full name").fill("Ada Lovelace");
    await page.getByLabel("Street address", { exact: true }).fill("1 rue de la Paix");
    await page.getByLabel("Postal code", { exact: true }).fill("75002");
    await page.getByLabel("City", { exact: true }).fill("Paris");
    await page.getByLabel("Country", { exact: true }).selectOption("FR");
    await page.getByRole("button", { name: "Place order" }).click();
    const guestOrder = await pay(page);

    // The shop mailed its confirmation, with a link back to the order.
    const guestOrderId = new URL(guestOrder).pathname.split("/").at(-1)!;
    await expect(async () => {
      const all = (await (await request.get(`${MAILDEV}/email`)).json()) as {
        to: { address: string }[];
        subject: string;
        from: { name: string }[];
        html?: string;
      }[];
      const confirmation = all.find(
        (m) => m.to.some((to) => to.address === shopper) && m.subject.startsWith(`Your ${name} order `),
      );
      expect(confirmation).toBeDefined();
      expect(confirmation!.from[0]?.name).toBe(name);
      expect(confirmation!.html).toContain(`${STOREFRONT}/orders/${guestOrderId}`);
      expect(confirmation!.html).toContain("Blue Mug");
    }).toPass({ timeout: 15_000 });
    await page.goto(`/t/${slug}/orders/${guestOrderId}`);
    await expect(page.locator("[data-order-confirmation]")).toHaveAttribute("data-order-confirmation", "mailed");

    // Next time, the checkout offers to sign in, and comes back to it.
    await page.goto(`${STOREFRONT}/p/red-plate`);
    await addToCart(page);
    await page.goto(`${STOREFRONT}/checkout`);
    await expect(page.locator("[data-signed-in]")).toHaveCount(0);
    await page.locator("[data-returning]").getByRole("link", { name: "Click here to log in" }).click();
    await expect(page).toHaveURL(/\/account\/sign-in\?/);
    await signInToShop(page, shopper);
    await expect(page).toHaveURL(`${STOREFRONT}/checkout`);
    await expect(page.locator("[data-signed-in]")).toContainText(shopper);
    await expect(page.locator("[data-returning]")).toHaveCount(0);
    await expect(page.getByLabel("E-mail")).toHaveValue(shopper);
    await expect(page.getByLabel("E-mail")).toHaveAttribute("readonly", "");
    await expect(page.getByLabel("City", { exact: true })).toHaveValue("Paris");
    await expect(page.getByLabel("Save this address to my account")).toBeChecked();
    // Billed to a company this time, kept on the account too.
    await page.getByLabel("Use a different billing address (optional)").check();
    await page.getByLabel("Billing name or company").fill("Analytical Engines GmbH");
    await page.getByLabel("Billing street address").fill("Unter den Linden 1");
    await page.getByLabel("Billing city").fill("Berlin");
    await page.getByLabel("Billing postal code").fill("10117");
    await page.getByLabel("Billing country").selectOption("DE");
    await expect(page.getByLabel("Save this billing address to my account")).not.toBeChecked();
    await page.getByLabel("Save this billing address to my account").check();
    await page.getByRole("button", { name: "Place order" }).click();
    await pay(page);

    // The code came from the shop.
    const mail = (await (await request.get(`${MAILDEV}/email`)).json()) as {
      to: { address: string }[];
      subject: string;
      from: { name: string }[];
    }[];
    const sent = mail.filter(
      (m) => m.to.some((to) => to.address === shopper) && m.subject.startsWith("Your sign-in code"),
    );
    expect(sent.at(-1)?.subject).toBe(`Your sign-in code for ${name}`);
    expect(sent.at(-1)?.from[0]?.name).toBe(name);

    // Another device: a wrong code is refused, the right one shows both
    // orders (the guest one too) and the address kept at checkout.
    const other = await browser.newContext();
    try {
      const device = await other.newPage();
      await fakeStripeJs(device);
      await device.goto(`${STOREFRONT}/account`);
      await expect(device).toHaveURL(/\/account\/sign-in\?/);
      await signInToShop(device, shopper, "000000");
      await expect(device).toHaveURL(`${STOREFRONT}/account`);
      await expect(device.locator("[data-account-email]")).toHaveText(shopper);
      await expect(async () => {
        await device.reload();
        await expect(device.locator("[data-account-order]")).toHaveCount(2, { timeout: 1000 });
      }).toPass({ timeout: 15_000 });
      await expect(device.locator("[data-saved-address]")).toHaveCount(2);
      await expect(device.locator("[data-saved-address=Paris]")).toContainText("1 rue de la Paix");
      await expect(device.locator("[data-saved-address=Berlin]")).toContainText("Unter den Linden 1");

      // The guest order opens here: it was placed with this address.
      const guestPath = new URL(guestOrder).pathname;
      await device.locator(`a[href="${guestPath}"]`).click();
      await expect(device).toHaveURL(guestOrder);
      await expect(device.getByRole("heading", { level: 1 })).toBeVisible();

      // Its checkout starts from the saved address.
      await device.goto(`${STOREFRONT}/p/blue-mug`);
      await addToCart(device);
      await device.goto(`${STOREFRONT}/checkout`);
      await expect(device.getByLabel("E-mail")).toHaveValue(shopper);
      await expect(device.getByLabel("Full name")).toHaveValue("Ada Lovelace");
      await expect(device.getByLabel("Street address", { exact: true })).toHaveValue("1 rue de la Paix");
      await expect(device.getByLabel("City", { exact: true })).toHaveValue("Paris");
      await expect(device.locator("[data-saved-address=Paris]")).toHaveAttribute("data-current", "");

      // Billed to another saved address in one click; still shipped to Paris.
      await expect(device.locator("[data-billing]")).toBeHidden();
      await device.locator("[data-saved-address=Berlin]").getByRole("link", { name: "Bill here" }).click();
      await expect(device).toHaveURL(/[?&]billing=/);
      await expect(device.locator("[data-saved-address=Berlin]")).toHaveAttribute("data-billing-current", "");
      await expect(device.locator("[data-saved-address=Paris]")).toHaveAttribute("data-current", "");
      await expect(device.getByLabel("Use a different billing address (optional)")).toBeChecked();
      await expect(device.getByLabel("Billing name or company")).toHaveValue("Analytical Engines GmbH");
      await expect(device.getByLabel("Billing city")).toHaveValue("Berlin");
      await expect(device.getByLabel("Billing country")).toHaveValue("DE");
      await expect(device.getByLabel("City", { exact: true })).toHaveValue("Paris");
      // Shipping elsewhere keeps the billing choice.
      await device.locator("[data-saved-address=Berlin]").getByRole("link", { name: "Ship here" }).click();
      await expect(device.getByLabel("City", { exact: true })).toHaveValue("Berlin");
      await expect(device.getByLabel("Billing city")).toHaveValue("Berlin");

      // Signed out, an order this browser did not place asks to sign in
      // first (the confirmation's link, opened elsewhere), then shows.
      await device.goto(`${STOREFRONT}/account`);
      await device.getByRole("button", { name: "Sign out", exact: true }).click();
      await expect(device.locator("[data-account-link]")).toHaveText("Sign in");
      await device.goto(guestOrder);
      await expect(device).toHaveURL(/\/account\/sign-in\?/);
      await signInToShop(device, shopper);
      await expect(device).toHaveURL(guestOrder);
      await expect(device.getByRole("heading", { level: 1 })).toBeVisible();

      // Removing the addresses, back on the first browser.
      await page.goto(`${STOREFRONT}/account`);
      await page.locator("[data-saved-address=Paris]").getByRole("button", { name: "Remove" }).click();
      await page.locator("[data-saved-address=Berlin]").getByRole("button", { name: "Remove" }).click();
      await expect(page.locator("[data-saved-address]")).toHaveCount(0);

      // The merchant finds the shopper among the shop's customers, with both
      // orders (the guest one too) and no address left.
      await page.goto(`/t/${slug}/customers`);
      const customer = page.locator(`[data-customer="${shopper}"]`);
      await expect(async () => {
        await page.reload();
        await expect(customer.locator("[data-customer-addresses]")).toHaveText("0", { timeout: 1000 });
      }).toPass({ timeout: 15_000 });
      await expect(customer.locator("[data-customer-orders]")).toHaveText("2");
      await customer.getByRole("link", { name: "2" }).click();
      await expect(page).toHaveURL(new RegExp(`/t/${slug}/orders\\?q=`));
      await expect(page.locator("[data-order]")).toHaveCount(2);

      // Signing out everywhere else, from the first browser: the other device
      // is signed out, this one stays.
      await page.goto(`${STOREFRONT}/account`);
      await page.getByRole("button", { name: "Sign out everywhere else" }).click();
      await expect(page.getByRole("status")).toContainText("Every other browser is signed out.");
      await expect(page.locator("[data-account-email]")).toHaveText(shopper);
      await device.goto(`${STOREFRONT}/account`);
      await expect(device).toHaveURL(/\/account\/sign-in\?/);

      // Deleting the account: signed out, gone from the customers, while the
      // shop keeps both orders.
      await page.goto(`${STOREFRONT}/account`);
      await page.getByRole("link", { name: "Delete your account" }).click();
      await expect(page).toHaveURL(`${STOREFRONT}/account/delete`);
      await page.getByRole("button", { name: "Delete my account" }).click();
      await expect(page).toHaveURL(`${STOREFRONT}/`);
      await expect(page.locator("[data-account-link]")).toHaveText("Sign in");
      await page.goto(`/t/${slug}/customers`);
      await expect(async () => {
        await page.reload();
        await expect(page.locator(`[data-customer="${shopper}"]`)).toHaveCount(0, { timeout: 1000 });
      }).toPass({ timeout: 15_000 });
      await page.goto(`/t/${slug}/orders?q=${encodeURIComponent(shopper)}`);
      await expect(page.locator("[data-order]")).toHaveCount(2);

      // Signing in again starts a new account, with the orders and no address.
      await signInToShop(device, shopper);
      await expect(device).toHaveURL(`${STOREFRONT}/account`);
      await expect(device.locator("[data-account-order]")).toHaveCount(2);
      await expect(device.locator("[data-saved-address]")).toHaveCount(0);
    } finally {
      await other.close();
    }
  } finally {
    await stripe.stop();
  }
});

test("a buyer is mailed a link to review a fulfilled order, which opens it on another device", async ({
  page,
  request,
  browser,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Asking Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await startStorefront(request, await mintKey(page, slug, true), 1);

    // No storefront address yet: the Reviews page says no mail goes out.
    await page.goto(`/t/${slug}/reviews`);
    await expect(page.locator("[data-review-requests]")).toHaveAttribute("data-review-requests", "7");
    await expect(page.locator("[data-review-requests-no-storefront]")).toBeVisible();
    await page.goto(`/t/${slug}/api-keys`);
    await page.getByLabel("Where shoppers find the shop").fill(STOREFRONT);
    await page.getByRole("button", { name: "Save address" }).click();
    await expect(page.getByRole("status")).toContainText("Storefront address saved.");
    // Ask at once, so the test does not wait a week.
    await page.goto(`/t/${slug}/reviews`);
    await expect(page.locator("[data-review-requests-no-storefront]")).toHaveCount(0);
    await page.getByLabel("Days after fulfilment").fill("0");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status")).toContainText("Review requests saved.");
    await expect(page.locator("[data-review-requests]")).toHaveAttribute("data-review-requests", "0");

    // A guest order.
    const shopper = uniqueEmail("buyer");
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await addToCart(page);
    await page.goto(`${STOREFRONT}/checkout`);
    await page.getByLabel("E-mail").fill(shopper);
    await page.getByLabel("Full name").fill("Ada Lovelace");
    await page.getByLabel("Street address", { exact: true }).fill("1 rue de la Paix");
    await page.getByLabel("Postal code", { exact: true }).fill("75002");
    await page.getByLabel("City", { exact: true }).fill("Paris");
    await page.getByLabel("Country", { exact: true }).selectOption("FR");
    await page.getByRole("button", { name: "Place order" }).click();
    await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
    const intent = stripe.lastIntent()!;
    stripe.pay(intent.id);
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
    await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    const orderId = new URL(page.url()).pathname.split("/").at(-1)!;

    // Not asked while it is only paid; asked once it is fulfilled.
    await page.goto(`/t/${slug}/orders/${orderId}`);
    await expect(page.locator("[data-order-review-request]")).toHaveAttribute("data-order-review-request", "none");
    await page.getByRole("button", { name: "Mark as fulfilled" }).click();
    await expect(page.getByRole("status")).toContainText("Order marked as fulfilled.");
    let link = "";
    await expect(async () => {
      const all = (await (await request.get(`${MAILDEV}/email`)).json()) as {
        to: { address: string }[];
        subject: string;
        from: { name: string }[];
        html?: string;
      }[];
      const asked = all.find((m) => m.to.some((to) => to.address === shopper) && m.subject.startsWith("How was your"));
      expect(asked).toBeDefined();
      expect(asked!.from[0]?.name).toBe(name);
      expect(asked!.html).toContain("Blue Mug");
      link = asked!.html!.match(new RegExp(`${STOREFRONT}/reviews/[A-Za-z0-9_-]{43}`))![0];
    }).toPass({ timeout: 30_000 });
    await page.goto(`/t/${slug}/orders/${orderId}`);
    await expect(page.locator("[data-order-review-request]")).toHaveAttribute("data-order-review-request", "mailed");

    // Another device, never signed in: the link opens the order's review form.
    const other = await (await browser.newContext({ ignoreHTTPSErrors: true })).newPage();
    await other.goto(link);
    await expect(other).toHaveURL(`${STOREFRONT}/orders/${orderId}#reviews`);
    const mug = other.locator("[data-review-of]", { hasText: "Blue Mug" });
    await mug.getByRole("radio", { name: "5 stars" }).check();
    await mug.getByLabel("Your review").fill("Arrived quickly and holds a big tea.");
    await mug.getByRole("button", { name: "Send my review" }).click();
    await expect(mug.getByRole("status")).toContainText("Your review is with the shop.");

    // A token that opens nothing says so.
    await other.goto(`${STOREFRONT}/reviews/not-a-token`);
    await expect(other.locator("[data-review-link-expired]")).toBeVisible();
    await other.context().close();
  } finally {
    await stripe.stop();
  }
});
