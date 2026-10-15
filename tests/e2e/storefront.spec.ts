import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerWithEmail, uniqueEmail } from "./email";
import { FakeStripe, stripeSignature } from "./stripe";
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
/// tenant's pictures are served from. `sell` adds store:checkout to the
/// default read scope.
async function mintKey(page: Page, slug: string, sell = false): Promise<{ token: string; mediaUrl: string }> {
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
  return { token, mediaUrl };
}

/// Boots the example storefront with `token` and waits for it to have synced
/// at least `products` products.
async function startStorefront(
  request: APIRequestContext,
  { token, mediaUrl }: { token: string; mediaUrl: string },
  products: number,
) {
  dir = fs.mkdtempSync(path.join(os.tmpdir(), "timada-storefront-"));
  child = spawn(path.resolve("target/debug/storefront"), [], {
    env: {
      ...process.env,
      STORE_ADDR,
      STORE_TOKEN: token,
      STORE_MEDIA_URL: mediaUrl,
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

test("a storefront browses the categories its products are filed under", async ({ page, request }) => {
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
  await expect(async () => {
    await page.goto(`${STOREFRONT}/`);
    await expect(page.locator("nav.sections [data-category=bedding]")).toContainText("Bedding (2)", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("nav.sections [data-category=mats]")).toContainText("Mats (2)");
  await expect(page.locator("nav.sections [data-category=toys]")).toContainText("Toys (1)");

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
  await page.getByRole("button", { name: "Add to cart" }).click();
  await expect(page).toHaveURL(`${STOREFRONT}/cart`);
  await expect(page.locator("[data-cart-total]")).toHaveText("25.00 EUR");
  await page.goto(`${STOREFRONT}/p/red-plate`);
  await page.getByRole("button", { name: "Add to cart" }).click();
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

  // Checkout: a bad country is refused at once and the form comes back
  // filled in.
  await page.goto(`${STOREFRONT}/cart`);
  await page.getByRole("link", { name: "Check out" }).click();
  await expect(page.locator("[data-checkout-total]")).toHaveText("37.50 EUR");
  await page.getByLabel("E-mail").fill("ada@example.org");
  await page.getByLabel("Full name").fill("Ada Lovelace");
  await page.getByLabel("Street address").fill("1 rue de la Paix");
  await page.getByLabel("Postal code").fill("75002");
  await page.getByLabel("City").fill("Paris");
  await page.getByLabel("Country code").fill("F1");
  await page.getByRole("button", { name: "Continue to payment" }).click();
  await expect(page.getByRole("alert")).toContainText("two-letter code");
  await expect(page.getByLabel("Full name")).toHaveValue("Ada Lovelace");

  // The request to pay is refused: the shop has not said who it is yet, so
  // it could not issue the invoice.
  await page.getByLabel("Country code").fill("FR");
  await page.getByRole("button", { name: "Continue to payment" }).click();
  await expect(page.getByRole("alert")).toContainText("cannot take orders yet", { timeout: 30_000 });
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

/// From the cart to the card form: the checkout form filled (or kept from
/// the last order) and sent.
async function continueToPayment(page: Page, fill: boolean) {
  await page.goto(`${STOREFRONT}/cart`);
  await page.getByRole("link", { name: "Check out" }).click();
  if (fill) {
    await page.getByLabel("E-mail").fill("ada@example.org");
    await page.getByLabel("Full name").fill("Ada Lovelace");
    await page.getByLabel("Street address").fill("1 rue de la Paix");
    await page.getByLabel("Postal code").fill("75002");
    await page.getByLabel("City").fill("Paris");
    await page.getByLabel("Country code").fill("FR");
  } else {
    await expect(page.getByLabel("Full name")).toHaveValue("Ada Lovelace");
  }
  await page.getByRole("button", { name: "Continue to payment" }).click();
  await expect(page).toHaveURL(`${STOREFRONT}/checkout/pay`);
}

test("a shopper pays with Stripe and the merchant fulfils or refunds the order", async ({ page, request }) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Paying Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await publishProduct(page, slug, "Red Plate", "8");

    // The merchant says who it is, so its invoices can name it; a SIRET
    // that fails its check is refused.
    await page.goto(`/t/${slug}/invoicing`);
    await expect(page.locator("[data-invoicing]")).toHaveAttribute("data-invoicing", "none");
    await setLegalIdentity(page, slug);
    await page.getByLabel("SIRET").fill("12002701600093");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page).toHaveURL(/error=invalid_siret/);
    await expect(page.locator("[data-invoicing]")).toHaveCount(0);

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

    // Two mugs, then the card form with the payment the dashboard opened.
    await page.goto(`${STOREFRONT}/p/blue-mug`);
    await page.getByLabel("Quantity").fill("2");
    await page.getByRole("button", { name: "Add to cart" }).click();
    await continueToPayment(page, true);
    const intent = stripe.lastIntent()!;
    expect(intent.amount).toBe(2500);
    expect(intent.currency).toBe("eur");
    const form = page.locator("[data-payment-form]");
    await expect(form).toHaveAttribute("data-publishable-key", "pk_test_e2e");
    await expect(form).toHaveAttribute("data-client-secret", `${intent.id}_secret_e2e`);
    await expect(page.locator("[data-payment-total]")).toHaveText("25.00 EUR");

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
    await expect(page.locator("[data-order-address]")).toContainText("Ada Lovelace");
    await page.getByLabel("Tracking number (optional)").fill("6A12345");
    await page.getByRole("button", { name: "Mark as fulfilled" }).click();
    await expect(page.getByRole("status")).toContainText("Order marked as fulfilled.");
    await expect(page.locator("[data-order-tracking]")).toHaveText("6A12345");
    await expect(page.getByRole("button", { name: "Cancel and refund" })).toHaveCount(0);

    // A second order is cancelled: the money goes back through Stripe.
    await page.goto(`${STOREFRONT}/p/red-plate`);
    await page.getByRole("button", { name: "Add to cart" }).click();
    await continueToPayment(page, false);
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
  } finally {
    await stripe.stop();
  }
});
