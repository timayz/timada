import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerWithEmail, uniqueEmail } from "./email";
import { FakeStripe, fakeStripeJs, stripeSignature } from "./stripe";
import { createTenant, setLegalIdentity, uniqueTenant } from "./tenants";

// Checkout offers: a merchant offers one unit of a product at a lower price
// while the cart holds another one (/t/{slug}/offers); the example
// storefront shows it as an "Exclusive offer" card at the checkout, the
// shopper adds it there, and the total, the payment and the order follow.
// The storefront runs on a port of its own, so this file can run beside
// `storefront.spec.ts`.

const STORE_ADDR = process.env.E2E_STORE_ADDR ?? "127.0.0.1:4321";
const STOREFRONT_PORT = Number(process.env.E2E_OFFERS_STOREFRONT_PORT ?? 3457);
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

/// Mints a key that may sell and boots the example storefront with it,
/// waiting for it to have synced `products` products.
async function startStorefront(page: Page, request: APIRequestContext, slug: string, products: number) {
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Label").fill("Example storefront");
  await page.getByRole("checkbox", { name: "store:checkout" }).first().check();
  await page.getByRole("button", { name: "Create key" }).click();
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_/);
  const mediaUrl = (await page.locator("[data-media-url]").textContent())?.trim() ?? "";
  const apiUrl = (await page.locator("[data-api-url]").textContent())?.trim() ?? "";
  dir = fs.mkdtempSync(path.join(os.tmpdir(), "timada-storefront-"));
  child = spawn(path.resolve("target/debug/storefront"), [], {
    env: {
      ...process.env,
      STORE_ADDR,
      STORE_TOKEN: token,
      STORE_MEDIA_URL: mediaUrl,
      STORE_API_URL: apiUrl,
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

/// Creates a published product priced at `amount`.
async function publishProduct(page: Page, slug: string, name: string, amount: string) {
  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  await page.getByLabel("Price").fill(amount);
  await page.getByRole("button", { name: "Set price" }).click();
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");
}

/// Opens the offers page once the products' read model lists `item` in
/// the picker: it is a moment behind the write.
async function openOffers(page: Page, slug: string, item: string) {
  await expect(async () => {
    await page.goto(`/t/${slug}/offers`);
    await expect(page.getByLabel("Item").locator("option", { hasText: item })).toHaveCount(1, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
}

/// Adds the product page's pick to the cart: the cart drawer opens on it.
async function addToCart(page: Page) {
  await page.getByRole("button", { name: "Add to cart" }).click();
  await expect(page.locator("[data-cart-drawer].open .drawer-panel")).toBeVisible();
}

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

test("a shopper adds the merchant's exclusive offer at the checkout and pays its price", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Offer Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Leather Collar", "20.00");
    await publishProduct(page, slug, "GPS Tracker", "30.00");
    await setLegalIdentity(page, slug);
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");

    // An offer is below the product's price.
    await page.goto(`/t/${slug}`);
    await page.getByRole("link", { name: "Offers", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${slug}/offers$`));
    await openOffers(page, slug, "GPS Tracker");
    await expect(page.getByText("No offer yet.")).toBeVisible();
    await page.getByLabel("Item").selectOption({ label: "GPS Tracker (30.00 EUR)" });
    await page.getByLabel("Offer price").fill("30.00");
    await page.getByRole("button", { name: "Create offer" }).click();
    await expect(page.getByRole("alert").first()).toContainText("below the product's price");

    // One tracker at 12.00 while the cart holds a collar.
    await page.getByLabel("Item").selectOption({ label: "GPS Tracker (30.00 EUR)" });
    await page.getByLabel("Offer price").fill("12.00");
    await page.getByLabel("Title (optional)").fill("Add a smart GPS tracker at 60% off");
    const requires = page.getByLabel("Only when the cart holds (optional)");
    await requires.selectOption({ label: "Leather Collar" });
    await page.getByRole("button", { name: "Create offer" }).click();
    await expect(page.getByRole("status")).toContainText("Offer created.");
    const row = page.locator('[data-offer="GPS Tracker"]');
    await expect(row.locator("[data-offer-price]")).toHaveText("12.00 EUR");
    await expect(row.locator("[data-offer-conditions]")).toContainText("Cart holds Leather Collar");
    await expect(row.locator("[data-offer-status]")).toHaveAttribute("data-offer-status", "active");

    await startStorefront(page, request, slug, 2);

    // A tracker alone gets no offer.
    await page.goto(`${STOREFRONT}/p/gps-tracker`);
    await addToCart(page);
    await page.goto(`${STOREFRONT}/checkout`);
    await expect(page.locator("[data-checkout-offers]")).toHaveCount(0);
    await page.getByRole("button", { name: "Remove GPS Tracker" }).click();

    // With a collar, the checkout offers the tracker; added there, one is
    // priced at the offer.
    await page.goto(`${STOREFRONT}/p/leather-collar`);
    await addToCart(page);
    await page.goto(`${STOREFRONT}/checkout`);
    const offers = page.locator("[data-checkout-offers]");
    await expect(offers.getByRole("heading", { name: "Exclusive offer" })).toBeVisible();
    const card = offers.locator('[data-checkout-offer="GPS Tracker"]');
    await expect(card).toContainText("Add a smart GPS tracker at 60% off");
    await expect(card.locator("[data-offer-price]")).toHaveText("12.00 EUR");
    await expect(page.locator("[data-checkout-total]")).toHaveText("20.00 EUR");
    await card.getByRole("button", { name: "Add GPS Tracker at 12.00 EUR" }).click();
    await expect(page).toHaveURL(`${STOREFRONT}/checkout`);
    await expect(page.locator("[data-checkout-offers]")).toHaveCount(0);
    const line = page.locator('[data-summary-line="GPS Tracker"]');
    await expect(line.locator("[data-line-offer]")).toContainText("1 at 12.00 EUR");
    await expect(page.locator("[data-checkout-subtotal]")).toHaveText("32.00 EUR");
    await expect(page.locator("[data-checkout-total]")).toHaveText("32.00 EUR");

    // A second tracker is at full price.
    await line.getByRole("button", { name: "One more GPS Tracker" }).click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("62.00 EUR");
    await line.getByRole("button", { name: "One less GPS Tracker" }).click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("32.00 EUR");

    // Charged what the page showed; the order says what the tracker cost.
    await page.getByLabel("E-mail").fill("ada@example.org");
    await page.getByLabel("Full name").fill("Ada Lovelace");
    await page.getByLabel("Street address", { exact: true }).fill("1 rue de la Paix");
    await page.getByLabel("Postal code", { exact: true }).fill("75002");
    await page.getByLabel("City", { exact: true }).fill("Paris");
    await page.getByLabel("Country", { exact: true }).selectOption("FR");
    await page.getByRole("button", { name: "Place order" }).click();
    await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
    const intent = stripe.lastIntent()!;
    expect(intent.amount).toBe(3200);
    stripe.pay(intent.id);
    expect((await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")).status()).toBe(204);
    await page.goto(`${STOREFRONT}/checkout/complete?redirect_status=succeeded`);
    await expect(page).toHaveURL(new RegExp(`${STOREFRONT}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
    await expect(page.locator("[data-order-total]")).toHaveText("32.00 EUR");

    // Deactivated, the offer is no longer shown.
    await page.goto(`/t/${slug}/offers`);
    await page.getByRole("button", { name: "Deactivate the offer on GPS Tracker" }).click();
    await expect(page.getByRole("status")).toContainText("Offer deactivated.");
    await expect(row.locator("[data-offer-status]")).toHaveAttribute("data-offer-status", "inactive");
    await page.goto(`${STOREFRONT}/p/leather-collar`);
    await addToCart(page);
    await page.goto(`${STOREFRONT}/checkout`);
    await expect(page.locator("[data-checkout-offers]")).toHaveCount(0);

    // Archived, it leaves the list.
    await page.goto(`/t/${slug}/offers`);
    await page.getByRole("button", { name: "Archive the offer on GPS Tracker" }).click();
    await expect(page.getByRole("status")).toContainText("Offer archived.");
    await expect(page.getByText("No offer yet.")).toBeVisible();
  } finally {
    await stripe.stop();
  }
});
