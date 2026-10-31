import { expect, test, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import {
  addToCart,
  confirmPayment,
  connectStripe,
  fillCheckout,
  payButton,
  paymentSucceeded,
  placeOrder,
} from "./shop";
import { ExampleApp, PNG, mintKey, publishProduct } from "./storefront";
import { FakeStripe, fakeStripeJs } from "./stripe";
import { createTenant, setLegalIdentity, uniqueTenant } from "./tenants";

// The shop as a shopper sees it, on the Chatounette example (the app on the
// storefront library): the catalogue following the dashboard live, the
// categories and their menu, the cart and its drawer, the one-page checkout,
// paying with Stripe (a fake Stripe.js in the page, a fake Stripe API behind
// the dashboard), stock, discount codes, delivery methods and the merchant's
// checkout offers. The merchant's side is the dashboard, in English; the shop
// is in French. The app runs on a port of its own, so this file runs beside
// the other Chatounette specs.

const app = new ExampleApp(
  "chatounette",
  Number(process.env.E2E_SHOP_PORT ?? 3459),
);
const SHOP = app.url;

test.beforeEach(async ({ page }) => {
  await fakeStripeJs(page);
});

test.afterEach(() => app.stop());

test("the shop serves the tenant's published catalogue with its key", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Mug Shop");
  await createTenant(page, name);

  // A product, priced, described, pictured and published.
  await page.getByRole("link", { name: "Products", exact: true }).click();
  await page.getByLabel("Name").fill("Blue Mug");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  await page.getByLabel("Price").fill("12.50");
  await page.getByRole("button", { name: "Set price" }).click();
  await page.getByLabel("Description").fill("Holds 50 cl of anything hot.");
  await page.getByRole("button", { name: "Save description" }).click();
  await page
    .getByLabel("Add a picture")
    .setInputFiles({ name: "mug.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill("A blue mug");
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute(
    "data-product-status",
    "published",
  );
  const productUrl = page.url().split("?")[0];

  // A key, shown once; the shop boots with it and syncs the catalogue into
  // its own file.
  const key = await mintKey(page, slug);
  await app.start(request, key, 1);

  await page.goto(`${SHOP}/boutique`);
  const card = page.locator("[data-product=blue-mug]");
  await expect(card).toContainText("Blue Mug");
  await expect(card.locator("[data-price]")).toHaveText("12,50€");
  await expect(card.locator("img.thumb")).toHaveAttribute("alt", "A blue mug");
  await card.getByRole("link", { name: "Blue Mug" }).first().click();
  await expect(page).toHaveURL(`${SHOP}/p/blue-mug`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Blue Mug");
  await expect(page.locator("p.description")).toContainText("Holds 50 cl");

  // The picture is loaded from the dashboard's media address, not the
  // shop's own, and the browser really fetches it.
  const photo = page.locator("[data-gallery] img").first();
  await expect(photo).toHaveAttribute("alt", "A blue mug");
  await expect(photo).toHaveAttribute("src", /timada\.localhost\/media\//);
  await expect(photo).toHaveJSProperty("naturalWidth", 1);
  expect((await page.goto(`${SHOP}/p/nothing-here`))?.status()).toBe(404);

  // Taken off sale in the dashboard, gone from the shop.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Unpublish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute(
    "data-product-status",
    "draft",
  );
  await expect(async () => {
    expect((await page.goto(`${SHOP}/p/blue-mug`))?.status()).toBe(404);
  }).toPass({ timeout: 15_000 });
  await page.goto(`${SHOP}/boutique`);
  await expect(page.locator("[data-product=blue-mug]")).toHaveCount(0);
});

test("the shop browses the categories its products are filed under", async ({
  page,
  request,
  browser,
}) => {
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
    await expect(page.locator("[data-category=bedding]")).toBeVisible({
      timeout: 1000,
    });
  }).toPass({ timeout: 15_000 });
  await page.getByLabel("Name").fill("Mats");
  const parent = page.getByLabel("Parent");
  await parent.selectOption(
    (await parent
      .locator("option", { hasText: "Bedding" })
      .first()
      .getAttribute("value")) ?? "",
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
  await app.start(request, key, 3);

  // The menu carries the tree with its counts; Bedding counts its subtree
  // and opens a panel led by "Tout Bedding"; Toys has nothing under it.
  const menu = page.locator("nav#menu");
  await expect(async () => {
    await page.goto(`${SHOP}/`);
    await expect(menu.locator("[data-category=bedding]")).toHaveAttribute(
      "data-products",
      "2",
      { timeout: 1000 },
    );
  }).toPass({ timeout: 15_000 });
  await expect(menu.locator("[data-category=mats]")).toHaveAttribute(
    "data-products",
    "2",
  );
  await expect(menu.locator("[data-category=toys]")).toHaveAttribute(
    "data-products",
    "1",
  );
  await expect(menu.getByRole("link", { name: "Nos produits" })).toBeVisible();
  await expect(menu.locator("[data-menu=bedding] a.menu-all")).toHaveText(
    "Tout Bedding",
  );
  await expect(menu.locator("[data-category=toys]")).toBeVisible();

  // The shop and the back office agree on every one of those numbers.
  await page.goto(`/t/${slug}/categories`);
  for (const [category, count] of [
    ["bedding", "2"],
    ["mats", "2"],
    ["toys", "1"],
  ]) {
    await expect(
      page.locator(`[data-category=${category}] [data-category-count]`),
    ).toHaveText(count);
  }
  await expect(
    page.locator("[data-category=bedding] [data-category-direct]"),
  ).toHaveAttribute("data-category-direct", "0");

  // A top-level category lists what is under it, newest first by default.
  await page.goto(`${SHOP}/c/bedding`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Bedding");
  await expect(page.locator("[data-subcategory=mats]")).toContainText(
    "Mats (2)",
  );
  await expect(page.locator("ul.catalog li")).toHaveCount(2);
  await expect(page.locator("[data-product=feather-wand]")).toHaveCount(0);

  // Sorting is a plain GET form.
  await page.getByLabel("Trier").selectOption("price_asc");
  await page.getByRole("button", { name: "Trier" }).click();
  await expect(page).toHaveURL(/sort=price_asc/);
  await expect(page.locator("ul.catalog li").first()).toContainText("Wool Mat");
  await page.getByLabel("Trier").selectOption("price_desc");
  await page.getByRole("button", { name: "Trier" }).click();
  await expect(page.locator("ul.catalog li").first()).toContainText(
    "Cooling Mat",
  );

  // A sub-category page carries a breadcrumb up to its parent.
  await page.goto(`${SHOP}/c/mats`);
  await expect(page.locator("[data-crumbs]")).toContainText("Bedding");
  await expect(page.locator("[data-crumbs]")).toContainText("Mats");
  expect((await page.goto(`${SHOP}/c/nothing-here`))?.status()).toBe(404);

  // A product page carries its category and what else is in it.
  await page.goto(`${SHOP}/p/cooling-mat`);
  await expect(page.locator("[data-crumbs]")).toContainText("Mats");
  const related = page.locator("[data-section=similaires]");
  await expect(related).toContainText("Produits similaires");
  await expect(related.locator("[data-product=wool-mat]")).toBeVisible();
  await expect(related.locator("[data-product=cooling-mat]")).toHaveCount(0);

  // On a phone without JS, "Menu" opens the whole tree as a sheet; a
  // category with children unfolds on its name.
  const phone = await browser.newContext({
    javaScriptEnabled: false,
    viewport: { width: 390, height: 800 },
  });
  const small = await phone.newPage();
  await small.goto(`${SHOP}/`);
  await expect(small.locator("nav#menu")).toBeHidden();
  await small.getByRole("link", { name: "Menu", exact: true }).click();
  await expect(small).toHaveURL(/#menu$/);
  await expect(small.locator("nav#menu")).toBeVisible();
  await small.locator("nav#menu").getByText("Bedding", { exact: true }).click();
  await small.locator("nav#menu [data-category=mats]").click();
  await expect(small).toHaveURL(`${SHOP}/c/mats`);
  await phone.close();

  // A category the merchant hides leaves the menu, with what is under it;
  // its page still answers.
  await page.goto(`/t/${slug}/categories`);
  await page.locator("[data-category=bedding] a").first().click();
  await page.getByLabel("Show in the shop menu").uncheck();
  await page.getByRole("button", { name: "Save menu visibility" }).click();
  await expect(page.getByRole("status")).toContainText("Hidden from the menu.");
  await expect(async () => {
    await page.goto(`${SHOP}/`);
    await expect(page.locator("nav#menu [data-category=toys]")).toBeVisible({
      timeout: 1000,
    });
    await expect(page.locator("nav#menu [data-menu=bedding]")).toHaveCount(0, {
      timeout: 1000,
    });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("nav#menu [data-category=mats]")).toHaveCount(0);
  await page.goto(`${SHOP}/c/mats`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Mats");
  await expect(page.locator("[data-crumbs]")).toContainText("Bedding");
});

test("a shopper fills a cart and checks out, refused while the shop takes no payments", async ({
  page,
  request,
}) => {
  test.setTimeout(60_000);
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Order Shop");
  await createTenant(page, name);
  await publishProduct(page, slug, "Blue Mug", "12.50");
  await publishProduct(page, slug, "Red Plate", "8");

  const key = await mintKey(page, slug, true);
  await app.start(request, key, 2);

  // Into the cart, from two product pages.
  await page.goto(`${SHOP}/p/blue-mug`);
  await page.getByLabel("Quantité").fill("2");
  await addToCart(page);
  await expect(page).toHaveURL(`${SHOP}/p/blue-mug`);
  await expect(page.locator("[data-drawer-total]")).toHaveText("25.00 EUR");
  await page.goto(`${SHOP}/p/red-plate`);
  await addToCart(page);
  await expect(page.locator("[data-drawer-total]")).toHaveText("33.00 EUR");
  await page.goto(`${SHOP}/cart`);
  // The header carries the total too; the page's is the one in `main`.
  const total = page.locator("main [data-cart-total]");
  await expect(total).toHaveText("33.00 EUR");

  // Change a quantity, then take a line out.
  await page.getByLabel("Quantité de Blue Mug").fill("3");
  await page
    .locator("[data-cart-line='Blue Mug']")
    .getByRole("button", { name: "Mettre à jour" })
    .click();
  await expect(total).toHaveText("45.50 EUR");
  await page.getByRole("button", { name: "Retirer Red Plate" }).click();
  await expect(page.locator("[data-cart-line='Red Plate']")).toHaveCount(0);
  await expect(total).toHaveText("37.50 EUR");

  // An open cart shows in the dashboard.
  await expect(async () => {
    await page.goto(`/t/${slug}/carts`);
    await expect(page.locator("[data-cart-items]").first()).toHaveText("3", {
      timeout: 1000,
    });
  }).toPass({ timeout: 15_000 });

  // Checkout: one page, the cart beside the form. The shop takes no card
  // payments, so there is no card form.
  await page.goto(`${SHOP}/cart`);
  await page.getByRole("link", { name: "Commander" }).click();
  await expect(page.locator("ol.steps [aria-current=step]")).toHaveText(
    "Livraison & paiement",
  );
  await expect(page.locator("[data-checkout-total]")).toHaveText("37.50 EUR");
  await expect(page.locator("[data-payments-off]")).toBeVisible();
  await fillCheckout(page, true);

  // The request to pay is refused, the form still filled in: the shop has
  // not said who it is yet, so it could not issue the invoice.
  await payButton(page).click();
  await expect(page.getByRole("alert")).toContainText(
    "ne peut pas encore prendre de commandes",
    { timeout: 30_000 },
  );
  await expect(page.getByLabel("Nom complet")).toHaveValue("Ada Lovelace");
  await expect(page.locator("[data-checkout-total]")).toHaveText("37.50 EUR");
});

test("a shopper pays with Stripe and the merchant fulfils or refunds the order", async ({
  page,
  request,
}) => {
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
    await expect(page.locator("[data-invoicing]")).toHaveAttribute(
      "data-invoicing",
      "none",
    );
    const company = await setLegalIdentity(page, slug);
    await expect(page.locator("[data-invoicing]")).toHaveAttribute(
      "data-invoicing",
      "linked",
    );
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
    await expect(page.locator("[data-payments]")).toHaveAttribute(
      "data-payments",
      "none",
    );
    const webhookUrl =
      (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    expect(webhookUrl).toMatch(/\/stripe\/webhooks\/[0-9A-Z]{26}$/);
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("not-a-key");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page).toHaveURL(/error=invalid_secret_key/);
    await connectStripe(page, slug);
    await expect(page.locator("[data-publishable-key]")).toHaveText(
      "pk_test_e2e",
    );

    const key = await mintKey(page, slug, true);
    await app.start(request, key, 2);

    // Two mugs. The checkout shows Stripe's form for the total before any
    // payment is open, and the VAT the total includes where the goods go.
    await page.goto(`${SHOP}/p/blue-mug`);
    await page.getByLabel("Quantité").fill("2");
    await addToCart(page);
    await page.goto(`${SHOP}/cart`);
    await page.getByRole("link", { name: "Commander" }).click();
    const form = page.locator("[data-checkout-form]");
    await expect(form).toHaveAttribute("data-publishable-key", "pk_test_e2e");
    expect(await page.evaluate(() => (window as any).__stripe)).toMatchObject({
      key: "pk_test_e2e",
      elements: { mode: "payment", amount: 2500, currency: "eur" },
      // Asked once, in the form: Stripe's card form asks no country again.
      payment: {
        fields: { billingDetails: "never" },
        wallets: { link: "never" },
      },
    });
    await expect(page.locator("[data-checkout-total]")).toHaveText("25.00 EUR");
    await expect(page.locator("[data-checkout-vat]")).toHaveText(
      "(dont 4.17 EUR VAT FR 20%)",
    );
    await page.getByLabel("Pays", { exact: true }).selectOption("US");
    await expect(page.locator("[data-checkout-vat]")).toBeHidden();
    await page.getByLabel("Pays", { exact: true }).selectOption("FR");
    await expect(page.locator("[data-checkout-vat]")).toBeVisible();

    // The cart changes from the checkout and stays there.
    await page.getByRole("button", { name: "Un de plus : Blue Mug" }).click();
    await expect(page).toHaveURL(`${SHOP}/checkout`);
    await expect(page.locator("[data-checkout-total]")).toHaveText("37.50 EUR");
    await page.getByRole("button", { name: "Un de moins : Blue Mug" }).click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("25.00 EUR");

    // Placing the order opens the payment and confirms it with Stripe.
    await fillCheckout(page, true);
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    const intent = stripe.lastIntent()!;
    expect(intent.amount).toBe(2500);
    expect(intent.currency).toBe("eur");
    expect(
      new URL(page.url()).searchParams.get("payment_intent_client_secret"),
    ).toBe(`${intent.id}_secret_e2e`);
    expect(
      JSON.parse(
        (await page.evaluate(() => sessionStorage.getItem("confirmed")))!,
      ),
    ).toEqual({
      billing_details: {
        name: "Ada Lovelace",
        email: "ada@example.org",
        phone: "",
        address: {
          line1: "1 rue de la Paix",
          line2: "",
          city: "Paris",
          postal_code: "75002",
          state: "",
          country: "FR",
        },
      },
    });

    // Stripe sends the browser back before it tells the dashboard: the page
    // waits. A webhook that is not Stripe's is refused.
    await page.goto(`${SHOP}/checkout/complete?redirect_status=succeeded`);
    await expect(page.locator("[data-payment-confirming]")).toBeVisible();
    stripe.pay(intent.id);
    expect(
      (
        await paymentSucceeded(request, webhookUrl, intent.id, "whsec_forged")
      ).status(),
    ).toBe(400);
    expect(
      (
        await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")
      ).status(),
    ).toBe(204);
    await expect(page).toHaveURL(new RegExp(`${SHOP}/orders/[0-9a-f]{64}$`), {
      timeout: 15_000,
    });
    const reference =
      (await page.locator("[data-order-reference]").textContent())?.trim() ??
      "";
    expect(reference).toMatch(/^[0-9A-Z]{4}-[0-9A-Z]{4}$/);
    await expect(page.locator("[data-order-status]")).toHaveAttribute(
      "data-order-status",
      "paid",
    );
    await expect(page.locator("[data-order-total]")).toHaveText("25.00 EUR");
    // The dashboard invoices it a moment later; the shopper downloads it.
    const year = new Date().getUTCFullYear();
    const invoiceLink = page.locator(`[data-document="F${year}-000001"]`);
    await expect(async () => {
      await page.reload();
      await expect(invoiceLink).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    const invoicePdf = await page.request.get(
      new URL((await invoiceLink.getAttribute("href"))!, page.url()).toString(),
    );
    expect(invoicePdf.status()).toBe(200);
    expect(invoicePdf.headers()["content-type"]).toBe("application/pdf");
    expect(invoicePdf.headers()["content-disposition"]).toContain(
      `F${year}-000001.pdf`,
    );
    expect((await invoicePdf.body()).subarray(0, 5).toString()).toBe("%PDF-");
    // Told twice, still one order.
    expect(
      (
        await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")
      ).status(),
    ).toBe(204);
    await page.goto(`${SHOP}/cart`);
    await expect(page.locator("[data-cart-empty]")).toBeVisible();

    // The merchant finds it paid and fulfils it.
    const row = page.locator(`[data-order="${reference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(row).toHaveAttribute("data-order-status", "paid", {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: reference }).click();
    await expect(page.locator("[data-order-payment]")).toContainText(intent.id);
    await expect(
      page.locator(`[data-order-documents] [data-document="F${year}-000001"]`),
    ).toBeVisible();
    await expect(page.locator("[data-order-address]")).toContainText(
      "Ada Lovelace",
    );
    await page.getByLabel("Tracking number (optional)").fill("6A12345");
    await page.getByRole("button", { name: "Mark as fulfilled" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Order marked as fulfilled.",
    );
    await expect(page.locator("[data-order-tracking]")).toHaveText("6A12345");
    await expect(
      page.getByRole("button", { name: "Cancel and refund" }),
    ).toHaveCount(0);

    // A second order is cancelled: the money goes back through Stripe.
    await page.goto(`${SHOP}/p/red-plate`);
    await addToCart(page);
    await placeOrder(page, SHOP, false);
    const second = stripe.lastIntent()!;
    expect(second.id).not.toBe(intent.id);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    const secondReference =
      (await page.locator("[data-order-reference]").textContent())?.trim() ??
      "";
    const secondRow = page.locator(`[data-order="${secondReference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(secondRow).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await secondRow.getByRole("link", { name: secondReference }).click();
    await page.getByLabel("Reason for cancelling").fill("Out of stock");
    await page.getByRole("button", { name: "Cancel and refund" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Order cancelled and refunded.",
    );
    await expect(page.locator("[data-order-refunded]")).toHaveText("8.00 EUR");
    await expect(page.locator("[data-order-cancel-reason]")).toHaveText(
      "Out of stock",
    );
    expect(stripe.refunds).toEqual([
      { payment_intent: second.id, amount: 800 },
    ]);

    // The refund is credited against the second invoice.
    const creditNote = page.locator(
      `[data-order-documents] [data-document="A${year}-000001"]`,
    );
    await expect(async () => {
      await page.reload();
      await expect(creditNote).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(
      page.locator(`[data-order-documents] [data-document="F${year}-000002"]`),
    ).toBeVisible();
    const creditPdf = await page.request.get(
      (await creditNote.locator("a").getAttribute("href"))!,
    );
    expect(creditPdf.headers()["content-type"]).toBe("application/pdf");

    // The accountant's list holds all three.
    await expect(async () => {
      await page.goto(`/t/${slug}/invoices`);
      await expect(page.locator("[data-document]")).toHaveCount(3, {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });
    await expect(
      page.locator(`[data-document="A${year}-000001"]`),
    ).toContainText("Credit note");
  } finally {
    await stripe.stop();
  }
});

test("a counted product sells no more than is left, and a cancelled order puts it back", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Stock Shop");
    await createTenant(page, name);
    const mugUrl = await publishProduct(page, slug, "Last Mug", "10");

    // Not counted, it sells without limit; counted, the page says how many.
    await page.goto(mugUrl);
    await expect(page.locator("[data-stock]")).toHaveAttribute(
      "data-stock",
      "untracked",
    );
    await page.getByLabel("On the shelf").fill("1");
    await page.getByRole("button", { name: "Set stock" }).click();
    await expect(page.getByRole("status")).toContainText("Stock set.");
    await expect(page.locator("[data-stock]")).toHaveAttribute(
      "data-stock",
      "1",
    );

    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);
    const key = await mintKey(page, slug, true);
    await app.start(request, key, 1);

    // The shop shows what is left, and refuses a cart that wants more.
    await expect(async () => {
      await page.goto(`${SHOP}/p/last-mug`);
      await expect(page.locator("p.stock")).toHaveAttribute("data-stock", "1", {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });
    await page.getByLabel("Quantité").fill("2");
    await page.getByRole("button", { name: "Ajouter au panier" }).click();
    await expect(page).toHaveURL(/error=out_of_stock/);
    await expect(page.getByRole("alert")).toContainText(
      "il n'en reste pas assez",
    );

    // The last one is paid for: the shelf is empty and the shop says so.
    await page.getByLabel("Quantité").fill("1");
    await addToCart(page);
    await placeOrder(page, SHOP, true);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    const reference =
      (await page.locator("[data-order-reference]").textContent())?.trim() ??
      "";
    await expect(async () => {
      await page.goto(mugUrl);
      await expect(page.locator("[data-stock]")).toHaveAttribute(
        "data-stock",
        "0",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
    await expect(async () => {
      await page.goto(`/t/${slug}/products`);
      await expect(
        page.locator("[data-product=last-mug] [data-product-stock]"),
      ).toHaveAttribute("data-product-stock", "sold_out", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(async () => {
      await page.goto(`${SHOP}/p/last-mug`);
      await expect(page.locator("p.stock")).toHaveAttribute(
        "data-stock",
        "sold-out",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
    await expect(
      page.getByRole("button", { name: "Ajouter au panier" }),
    ).toBeDisabled();

    // Cancelled, the order puts its mug back on the shelf.
    const row = page.locator(`[data-order="${reference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: reference }).click();
    await page
      .getByLabel("Reason for cancelling")
      .fill("Broken in the warehouse");
    await page.getByRole("button", { name: "Cancel and refund" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Order cancelled and refunded.",
    );
    await expect(async () => {
      await page.goto(mugUrl);
      await expect(page.locator("[data-stock]")).toHaveAttribute(
        "data-stock",
        "1",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
  } finally {
    await stripe.stop();
  }
});

test("the cart drawer: changed in place, with offers, a code and express pay", async ({
  page,
  browser,
  request,
}) => {
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
    await connectStripe(page, slug);
    await page.goto(`/t/${slug}/discounts`);
    await page.getByLabel("Code").fill("TEN");
    await page.getByLabel("Percentage or amount").fill("10");
    await page.getByRole("button", { name: "Create discount" }).click();
    await expect(page.getByRole("status")).toContainText("Discount created.");
    await app.start(request, await mintKey(page, slug, true), 2);

    // Added from the product page: the drawer slides in, the page stays.
    await page.goto(`${SHOP}/p/blue-mug`);
    await expect(page.locator("[data-cart-count]")).toBeHidden();
    await addToCart(page);
    await expect(page).toHaveURL(`${SHOP}/p/blue-mug`);
    const drawer = page.locator("[data-cart-drawer]");
    await expect(
      drawer.locator("[data-summary-line='Blue Mug']"),
    ).toBeVisible();
    await expect(page.locator("[data-cart-count]")).toHaveText("1");
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("12.50 EUR");

    // The stepper changes it in place.
    await drawer.getByRole("button", { name: "Un de plus : Blue Mug" }).click();
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("25.00 EUR");
    await expect(page.locator("[data-cart-count]")).toHaveText("2");
    await expect(page).toHaveURL(`${SHOP}/p/blue-mug`);

    // What goes with it, added in one click.
    const offers = drawer.locator("[data-upsell]");
    await expect(offers).toContainText("Vous aimerez aussi");
    await expect(offers.locator("[data-offer=blue-mug]")).toHaveCount(0);
    await offers.getByRole("button", { name: "Ajouter Red Plate" }).click();
    await expect(
      drawer.locator("[data-summary-line='Red Plate']"),
    ).toBeVisible();
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("33.00 EUR");
    await expect(drawer.locator("[data-upsell]")).toHaveCount(0);

    // A code, from the drawer.
    await drawer.getByText("Vous avez un code promo ?").click();
    await drawer.getByLabel("Code promo").fill("tem");
    await drawer.getByRole("button", { name: "Appliquer" }).click();
    await expect(drawer.locator("[data-cart-error]")).toContainText(
      "n'est pas valable",
    );
    await drawer.getByLabel("Code promo").fill("ten");
    await drawer.getByRole("button", { name: "Appliquer" }).click();
    await expect(drawer.locator("[data-drawer-discount=TEN]")).toHaveText(
      "-3.30 EUR",
    );
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("29.70 EUR");

    // Closed with Escape, opened again from the header.
    await page.keyboard.press("Escape");
    await expect(drawer.locator(".drawer-panel")).toBeHidden();
    await expect(
      page.getByRole("button", { name: "Ajouter au panier" }),
    ).toBeEnabled();
    await page.locator("[data-cart-link]").click();
    await expect(drawer.locator(".drawer-panel")).toBeVisible();
    await drawer.getByRole("button", { name: "Retirer Red Plate" }).click();
    await expect(drawer.locator("[data-summary-line='Red Plate']")).toHaveCount(
      0,
    );
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
        new Promise((resolve) =>
          (window as any).__stripe.on["expressCheckout:click"]({ resolve }),
        ),
    );
    expect(asked).toMatchObject({
      emailRequired: true,
      shippingAddressRequired: true,
      billingAddressRequired: true,
    });
    // Billed elsewhere than delivered: the order is asked for with both.
    // Not awaited in the page: paying navigates away.
    const started = page.waitForRequest(
      (req) => req.method() === "POST" && req.url().includes("/checkout/start"),
    );
    await page.evaluate(() => {
      void (window as any).__stripe.on["expressCheckout:confirm"]({
        billingDetails: {
          email: "ada@example.com",
          name: "Analytical Engines GmbH",
          address: {
            line1: "Unter den Linden 1",
            postal_code: "10117",
            city: "Berlin",
            country: "DE",
          },
        },
        shippingAddress: {
          name: "Ada Lovelace",
          address: {
            line1: "1 rue de la Paix",
            postal_code: "75002",
            city: "Paris",
            country: "FR",
          },
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
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    expect(stripe.lastIntent()!.amount).toBe(2250);

    // Without JavaScript the forms come back to the page, drawer open.
    const plain = await browser.newContext({ javaScriptEnabled: false });
    try {
      const noScript = await plain.newPage();
      await noScript.goto(`${SHOP}/p/red-plate`);
      await noScript.getByRole("button", { name: "Ajouter au panier" }).click();
      await expect(noScript).toHaveURL(`${SHOP}/p/red-plate#cart`);
      const panel = noScript.locator("[data-cart-drawer] .drawer-panel");
      await expect(panel).toBeVisible();
      await expect(panel.locator("[data-drawer-total]")).toHaveText("8.00 EUR");
      await panel
        .getByRole("button", { name: "Un de plus : Red Plate" })
        .click();
      await expect(noScript).toHaveURL(`${SHOP}/p/red-plate#cart`);
      await expect(panel.locator("[data-drawer-total]")).toHaveText(
        "16.00 EUR",
      );
      await panel.getByRole("button", { name: "Retirer Red Plate" }).click();
      await expect(panel.locator("[data-cart-empty]")).toBeVisible();
    } finally {
      await plain.close();
    }
  } finally {
    await stripe.stop();
  }
});

test("a shopper pays less with a discount code, once", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Promo Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);

    // 10 % off, once per shopper.
    await page.goto(`/t/${slug}/discounts`);
    await page.getByLabel("Code").fill("TEN");
    await page.getByLabel("Percentage or amount").fill("10");
    await page.getByLabel("Once per shopper e-mail").check();
    await page.getByRole("button", { name: "Create discount" }).click();
    await expect(page.getByRole("status")).toContainText("Discount created.");
    await app.start(request, await mintKey(page, slug, true), 1);

    // A typo is refused at once; the right code takes 2.50 off two mugs.
    await page.goto(`${SHOP}/p/blue-mug`);
    await page.getByLabel("Quantité").fill("2");
    await addToCart(page);
    await page.goto(`${SHOP}/cart`);
    await page.getByLabel("Code promo").fill("TEM");
    await page.getByRole("button", { name: "Appliquer" }).click();
    await expect(page.getByRole("alert").first()).toContainText(
      "n'est pas valable",
    );
    await page.getByLabel("Code promo").fill("ten");
    await page.getByRole("button", { name: "Appliquer" }).click();
    await expect(page.locator("[data-cart-subtotal]")).toHaveText("25.00 EUR");
    await expect(
      page.locator("[data-cart-discount=TEN] [data-discount-amount]"),
    ).toHaveText("-2.50 EUR");
    await expect(page.locator("main [data-cart-total]")).toHaveText(
      "22.50 EUR",
    );

    // Charged the discounted total, and the order and its invoice say so.
    await page.getByRole("link", { name: "Commander" }).click();
    await expect(page.locator("[data-checkout-discount=TEN]")).toHaveText(
      "-2.50 EUR",
    );
    await expect(page.locator("[data-checkout-total]")).toHaveText("22.50 EUR");
    await placeOrder(page, SHOP, true);
    expect(stripe.lastIntent()!.amount).toBe(2250);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    await expect(page.locator("[data-order-discount=TEN]")).toContainText(
      "2.50 EUR",
    );
    await expect(page.locator("[data-order-total]")).toHaveText("22.50 EUR");
    const year = new Date().getUTCFullYear();
    const invoiceLink = page.locator(`[data-document="F${year}-000001"]`);
    await expect(async () => {
      await page.reload();
      await expect(invoiceLink).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(
      page.locator("[data-documents] li", { has: invoiceLink }),
    ).toContainText("22,50");
    const reference =
      (await page.locator("[data-order-reference]").textContent())?.trim() ??
      "";

    // The same shopper cannot use it twice: back to the cart, told why.
    await page.goto(`${SHOP}/p/blue-mug`);
    await addToCart(page);
    await page.goto(`${SHOP}/cart`);
    await page.getByRole("link", { name: "Commander" }).click();
    // Entered from the checkout's summary this time.
    await page.getByText("Vous avez un code promo ?").click();
    await page.getByLabel("Code promo").fill("TEN");
    await page.getByRole("button", { name: "Appliquer" }).click();
    await expect(page).toHaveURL(`${SHOP}/checkout`);
    await expect(page.locator("[data-checkout-discount=TEN]")).toHaveText(
      "-1.25 EUR",
    );
    await payButton(page).click();
    await expect(page).toHaveURL(
      new RegExp(`${SHOP}/cart\\?error=discount_already_used`),
      { timeout: 30_000 },
    );
    await expect(page.getByRole("alert").first()).toContainText(
      "déjà utilisé ce code promo",
    );

    // The merchant sees the discount on the order and the use on the code.
    const row = page.locator(`[data-order="${reference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: reference }).click();
    await expect(page.locator("[data-order-discount=TEN]")).toContainText(
      "-2.50 EUR",
    );
    await page.goto(`/t/${slug}/discounts`);
    await expect(
      page.locator("[data-discount=TEN] [data-discount-orders]"),
    ).toHaveText("1");
  } finally {
    await stripe.stop();
  }
});

test("a shopper picks a delivery method that ships to their country, and pays for it", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Ship Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);

    // Tracked to France and Belgium, free from 50.00; VIP to France only.
    await page.goto(`/t/${slug}/shipping`);
    await page.getByLabel("Name", { exact: true }).fill("Tracked delivery");
    await page.getByLabel("Price").fill("4.90");
    await page.getByLabel("Free from (optional)").fill("50");
    await page.getByRole("checkbox", { name: "France FR" }).check();
    await page.getByRole("checkbox", { name: "Belgium BE" }).check();
    await page.getByRole("button", { name: "Add shipping method" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Shipping method added.",
    );
    await page.getByLabel("Name", { exact: true }).fill("VIP");
    await page.getByLabel("Price").fill("6.90");
    await page.getByRole("checkbox", { name: "France FR" }).check();
    await page.getByRole("button", { name: "Add shipping method" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Shipping method added.",
    );
    await app.start(request, await mintKey(page, slug, true), 1);

    // Two mugs: the first method is picked, its price on top.
    await page.goto(`${SHOP}/p/blue-mug`);
    await page.getByLabel("Quantité").fill("2");
    await addToCart(page);
    await page.goto(`${SHOP}/cart`);
    await page.getByRole("link", { name: "Commander" }).click();
    const tracked = page.getByRole("radio", { name: /Tracked delivery/ });
    const vip = page.getByRole("radio", { name: /VIP/ });
    await expect(tracked).toBeChecked();
    await expect(page.locator("[data-checkout-shipping]")).toHaveText(
      "4.90 EUR",
    );
    await expect(page.locator("[data-checkout-total]")).toHaveText("29.90 EUR");
    await expect(page.locator("[data-checkout-vat]")).toContainText(
      "VAT FR 20%",
    );
    expect(
      await page.evaluate(() => (window as any).__stripe.elements.amount),
    ).toBe(2990);

    // VIP costs more; Belgium has only the tracked parcel; the US nothing.
    await vip.check();
    await expect(page.locator("[data-checkout-shipping]")).toHaveText(
      "6.90 EUR",
    );
    await expect(page.locator("[data-checkout-total]")).toHaveText("31.90 EUR");
    expect(
      await page.evaluate(() => (window as any).__stripe.elements.amount),
    ).toBe(3190);
    await page.getByLabel("Pays", { exact: true }).selectOption("BE");
    await expect(vip).toBeHidden();
    await expect(tracked).toBeChecked();
    await expect(page.locator("[data-checkout-total]")).toHaveText("29.90 EUR");
    await page.getByLabel("Pays", { exact: true }).selectOption("US");
    await expect(tracked).toBeHidden();
    await expect(page.locator("[data-no-shipping]")).toBeVisible();
    await expect(page.locator("[data-checkout-total]")).toHaveText("25.00 EUR");

    // Back to France with VIP: charged on top, on the order and its invoice.
    await fillCheckout(page, true);
    await vip.check();
    await expect(page.locator("[data-checkout-total]")).toHaveText("31.90 EUR");
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    expect(stripe.lastIntent()!.amount).toBe(3190);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    await expect(page.locator("[data-order-shipping=VIP]")).toContainText(
      "6.90 EUR",
    );
    await expect(page.locator("[data-order-total]")).toHaveText("31.90 EUR");
    const year = new Date().getUTCFullYear();
    const invoiceLink = page.locator(`[data-document="F${year}-000001"]`);
    await expect(async () => {
      await page.reload();
      await expect(invoiceLink).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(
      page.locator("[data-documents] li", { has: invoiceLink }),
    ).toContainText("31,90");
    const reference =
      (await page.locator("[data-order-reference]").textContent())?.trim() ??
      "";

    // The merchant sees what shipping was paid.
    const row = page.locator(`[data-order="${reference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: reference }).click();
    await expect(page.locator("[data-order-shipping=VIP]")).toContainText(
      "6.90 EUR",
    );
  } finally {
    await stripe.stop();
  }
});

/// Opens the offers page once the products' read model lists `item` in
/// the picker: it is a moment behind the write.
async function openOffers(page: Page, slug: string, item: string) {
  await expect(async () => {
    await page.goto(`/t/${slug}/offers`);
    await expect(
      page.getByLabel("Item").locator("option", { hasText: item }),
    ).toHaveCount(1, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
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
    const webhookUrl = await connectStripe(page, slug);

    // An offer is below the product's price.
    await page.goto(`/t/${slug}`);
    await page.getByRole("link", { name: "Offers", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${slug}/offers$`));
    await openOffers(page, slug, "GPS Tracker");
    await expect(page.getByText("No offer yet.")).toBeVisible();
    await page
      .getByLabel("Item")
      .selectOption({ label: "GPS Tracker (30.00 EUR)" });
    await page.getByLabel("Offer price").fill("30.00");
    await page.getByRole("button", { name: "Create offer" }).click();
    await expect(page.getByRole("alert").first()).toContainText(
      "below the product's price",
    );

    // One tracker at 12.00 while the cart holds a collar.
    await page
      .getByLabel("Item")
      .selectOption({ label: "GPS Tracker (30.00 EUR)" });
    await page.getByLabel("Offer price").fill("12.00");
    await page
      .getByLabel("Title (optional)")
      .fill("Add a smart GPS tracker at 60% off");
    await page
      .getByLabel("Only when the cart holds (optional)")
      .selectOption({ label: "Leather Collar" });
    await page.getByRole("button", { name: "Create offer" }).click();
    await expect(page.getByRole("status")).toContainText("Offer created.");
    const row = page.locator('[data-offer="GPS Tracker"]');
    await expect(row.locator("[data-offer-price]")).toHaveText("12.00 EUR");
    await expect(row.locator("[data-offer-conditions]")).toContainText(
      "Cart holds Leather Collar",
    );
    await expect(row.locator("[data-offer-status]")).toHaveAttribute(
      "data-offer-status",
      "active",
    );

    await app.start(request, await mintKey(page, slug, true), 2);

    // A tracker alone gets no offer.
    await page.goto(`${SHOP}/p/gps-tracker`);
    await addToCart(page);
    await page.goto(`${SHOP}/checkout`);
    await expect(page.locator("[data-checkout-offers]")).toHaveCount(0);
    await page.getByRole("button", { name: "Retirer GPS Tracker" }).click();

    // With a collar, the checkout offers the tracker; added there, one is
    // priced at the offer.
    await page.goto(`${SHOP}/p/leather-collar`);
    await addToCart(page);
    await page.goto(`${SHOP}/checkout`);
    const offers = page.locator("[data-checkout-offers]");
    await expect(
      offers.getByRole("heading", { name: "Offre exclusive" }),
    ).toBeVisible();
    const card = offers.locator('[data-checkout-offer="GPS Tracker"]');
    await expect(card).toContainText("Add a smart GPS tracker at 60% off");
    await expect(card.locator("[data-offer-price]")).toHaveText("12.00 EUR");
    await expect(page.locator("[data-checkout-total]")).toHaveText("20.00 EUR");
    await card
      .getByRole("button", { name: "Ajouter GPS Tracker à 12.00 EUR" })
      .click();
    await expect(page).toHaveURL(`${SHOP}/checkout`);
    await expect(page.locator("[data-checkout-offers]")).toHaveCount(0);
    const line = page.locator('[data-summary-line="GPS Tracker"]');
    await expect(line.locator("[data-line-offer]")).toContainText(
      "1 à 12.00 EUR",
    );
    await expect(page.locator("[data-checkout-subtotal]")).toHaveText(
      "32.00 EUR",
    );
    await expect(page.locator("[data-checkout-total]")).toHaveText("32.00 EUR");

    // A second tracker is at full price.
    await line
      .getByRole("button", { name: "Un de plus : GPS Tracker" })
      .click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("62.00 EUR");
    await line
      .getByRole("button", { name: "Un de moins : GPS Tracker" })
      .click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("32.00 EUR");

    // Charged what the page showed; the order says what the tracker cost.
    await fillCheckout(page, true);
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    expect(stripe.lastIntent()!.amount).toBe(3200);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    await expect(page.locator("[data-order-total]")).toHaveText("32.00 EUR");

    // Deactivated, the offer is no longer shown.
    await page.goto(`/t/${slug}/offers`);
    await page
      .getByRole("button", { name: "Deactivate the offer on GPS Tracker" })
      .click();
    await expect(page.getByRole("status")).toContainText("Offer deactivated.");
    await expect(row.locator("[data-offer-status]")).toHaveAttribute(
      "data-offer-status",
      "inactive",
    );
    await page.goto(`${SHOP}/p/leather-collar`);
    await addToCart(page);
    await page.goto(`${SHOP}/checkout`);
    await expect(page.locator("[data-checkout-offers]")).toHaveCount(0);

    // Archived, it leaves the list.
    await page.goto(`/t/${slug}/offers`);
    await page
      .getByRole("button", { name: "Archive the offer on GPS Tracker" })
      .click();
    await expect(page.getByRole("status")).toContainText("Offer archived.");
    await expect(page.getByText("No offer yet.")).toBeVisible();
  } finally {
    await stripe.stop();
  }
});
