import { expect, test, type Page } from "@playwright/test";
import { createHash } from "node:crypto";
import { mailCountOf, mailTo, registerWithEmail, uniqueEmail } from "./email";
import { productTab } from "./forms";
import {
  addToCart,
  confirmPayment,
  connectStripe,
  fillCheckout,
  payButton,
  paymentSucceeded,
  placeOrder,
} from "./shop";
import { StorefrontApp, PNG, mintKey, publishProduct } from "./storefront";
import { FakeMeta } from "./meta";
import { FakeStripe, fakeStripeJs } from "./stripe";
import { createTenant, setLegalIdentity, uniqueTenant } from "./tenants";

// The shop as a shopper sees it, on the storefront: the catalogue following the dashboard live, the
// categories and their menu, the cart and its drawer, the one-page checkout,
// paying with Stripe (a fake Stripe.js in the page, a fake Stripe API behind
// the dashboard), stock, discount codes, delivery methods and the merchant's
// checkout offers. The merchant's side is the dashboard, in English; the shop
// is in French. The app runs on a port of its own, so this file runs beside
// the other storefront specs.

const app = new StorefrontApp(Number(process.env.E2E_SHOP_PORT ?? 3459));
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
  await page.getByLabel("Description", { exact: true }).fill("Holds 50 cl of anything hot.");
  await page.getByRole("button", { name: "Save description" }).click();
  await expect(page.getByRole("status")).toContainText("Description saved.");
  await productTab(page, "Pictures");
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

  // The shop and the back office agree on every one of those numbers. The
  // back office reads its own read model, which trails the store by a
  // second or so after a burst of writes (the shop above read the store
  // itself), so the page is loaded again until it has caught up.
  await expect(async () => {
    await page.goto(`/t/${slug}/categories`);
    for (const [category, count] of [
      ["bedding", "2"],
      ["mats", "2"],
      ["toys", "1"],
    ]) {
      await expect(
        page.locator(`[data-category=${category}] [data-category-count]`),
      ).toHaveText(count, { timeout: 1000 });
    }
  }).toPass({ timeout: 15_000 });
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
  await expect(page.locator("[data-drawer-total]")).toHaveText("25,00€");
  await page.goto(`${SHOP}/p/red-plate`);
  await addToCart(page);
  await expect(page.locator("[data-drawer-total]")).toHaveText("33,00€");
  await page.goto(`${SHOP}/cart`);
  // The header carries the total too; the page's is the one in `main`.
  const total = page.locator("main [data-cart-total]");
  await expect(total).toHaveText("33,00€");

  // Change a quantity, then take a line out.
  await page.getByLabel("Quantité de Blue Mug").fill("3");
  await page
    .locator("[data-cart-line='Blue Mug']")
    .getByRole("button", { name: "Mettre à jour" })
    .click();
  await expect(total).toHaveText("45,50€");
  await page.getByRole("button", { name: "Retirer Red Plate" }).click();
  await expect(page.locator("[data-cart-line='Red Plate']")).toHaveCount(0);
  await expect(total).toHaveText("37,50€");

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
  await expect(page.locator("[data-checkout-total]")).toHaveText("37,50€");
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
  await expect(page.locator("[data-checkout-total]")).toHaveText("37,50€");
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
    await expect(page.getByLabel("Kind of goods, for VAT")).toHaveValue("standard");
    await page.getByLabel("Kind of goods, for VAT").selectOption("food");
    await page.getByRole("button", { name: "Set kind" }).click();
    await expect(page.getByRole("status")).toContainText("Kind of goods set.");
    await expect(page.getByLabel("Kind of goods, for VAT")).toHaveValue("food");

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
    for (const event of [
      "payment_intent.amount_capturable_updated",
      "payment_intent.succeeded",
      "payment_intent.canceled",
      "charge.refunded",
    ]) {
      await expect(page.getByText(event, { exact: true })).toBeVisible();
    }
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
      // Held when the shopper pays, taken by the shop.
      elements: { mode: "payment", amount: 2500, currency: "eur", captureMethod: "manual" },
      // Asked once, in the form: Stripe's card form asks no country again.
      payment: {
        fields: { billingDetails: "never" },
        wallets: { link: "never" },
      },
    });
    await expect(page.locator("[data-checkout-total]")).toHaveText("25,00€");
    await expect(page.locator("[data-checkout-vat]")).toHaveText(
      "(dont 4,17€ VAT FR 20%)",
    );
    await page.getByLabel("Pays", { exact: true }).selectOption("US");
    await expect(page.locator("[data-checkout-vat]")).toBeHidden();
    await page.getByLabel("Pays", { exact: true }).selectOption("FR");
    await expect(page.locator("[data-checkout-vat]")).toBeVisible();

    // The cart changes from the checkout and stays there.
    await page.getByRole("button", { name: "Un de plus : Blue Mug" }).click();
    await expect(page).toHaveURL(`${SHOP}/checkout`);
    await expect(page.locator("[data-checkout-total]")).toHaveText("37,50€");
    await page.getByRole("button", { name: "Un de moins : Blue Mug" }).click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("25,00€");

    // The phone's calling code is picked from a searchable list of every
    // country, ticked once chosen; France to start with.
    await fillCheckout(page, true);
    const phoneCountry = page.getByRole("button", { name: /^Indicatif/ });
    await expect(phoneCountry).toContainText("+33");
    await phoneCountry.click();
    await page.getByRole("combobox", { name: "Rechercher un pays" }).fill("belg");
    await page.locator("#phone-country-list").getByRole("option", { name: /Belgique/ }).click();
    await expect(phoneCountry).toContainText("+32");
    await phoneCountry.click();
    await expect(page.locator("#phone-country-list").getByRole("option", { name: /Belgique/ })).toHaveAttribute("aria-selected", "true");
    await page.getByRole("combobox", { name: "Rechercher un pays" }).fill("+33");
    await page.keyboard.press("Enter");
    await expect(phoneCountry).toContainText("+33");

    // Placing the order opens the payment and confirms it with Stripe.
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    const intent = stripe.lastIntent()!;
    expect(intent.amount).toBe(2500);
    expect(intent.currency).toBe("eur");
    expect(intent.captureMethod).toBe("manual");
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
        phone: "+33 6 12 34 56 78",
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
    // waits, then waits for the money to be taken, as the shop holds the
    // mugs. A webhook that is not Stripe's is refused. (The page reloads
    // itself every two seconds: sent there again, the browser could drop the
    // test's navigation for the page's own.)
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
    expect(intent.captured).toBe(true);
    await expect(page.locator("[data-order-total]")).toHaveText("25,00€");
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
    // The invoice is listed from the invoicing read model, a moment later.
    await expect(async () => {
      await page.reload();
      await expect(
        page.locator(`[data-order-documents] [data-document="F${year}-000001"]`),
      ).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(page.locator("[data-order-address]")).toContainText(
      "Ada Lovelace",
    );
    const shippedBefore = await mailCountOf(
      request,
      "ada@example.org",
      "Shipped: your",
    );
    await page.getByLabel("Tracking number (optional)").fill("6A12345");
    await page.getByRole("button", { name: "Mark as fulfilled" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Order marked as fulfilled.",
    );
    // The page reads the read model, a moment behind the action.
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-order-tracking]")).toHaveText(
        "6A12345",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
    // No carrier page given: the number stands alone, linked nowhere.
    await expect(page.locator("[data-order-tracking] a")).toHaveCount(0);
    // The shopper is mailed the shipment, with the number and no tracking
    // site (this shop told no storefront address, so no order link either).
    const shipped = await mailTo(
      request,
      "ada@example.org",
      "Shipped: your",
      shippedBefore,
    );
    expect(shipped.html).toContain("6A12345");
    expect(shipped.html).not.toContain("Follow your parcel");
    await expect(async () => {
      await page.reload();
      await expect(
        page.locator('[data-order-mail="order_shipped"]'),
      ).toHaveAttribute("data-mailed", "mailed", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
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
    // Paid, not merely placed: the money is taken a moment after the order
    // exists, and the read model the list reads trails that again.
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(secondRow).toHaveAttribute("data-order-status", "paid", {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });
    await secondRow.getByRole("link", { name: secondReference }).click();
    await page.getByLabel("Reason for cancelling").fill("Out of stock");
    await page.getByRole("button", { name: "Cancel and refund" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Order cancelled and refunded.",
    );
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-order-refunded]")).toHaveText(
        "8.00 €",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
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
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("12,50€");

    // The stepper changes it in place.
    await drawer.getByRole("button", { name: "Un de plus : Blue Mug" }).click();
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("25,00€");
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
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("33,00€");
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
      "-3,30€",
    );
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("29,70€");

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
    await expect(drawer.locator("[data-drawer-total]")).toHaveText("22,50€");

    // Express pay: Stripe's buttons for the drawer's total, paid with what
    // the wallet hands over.
    expect(await page.evaluate(() => (window as any).__stripe)).toMatchObject({
      key: "pk_test_e2e",
      elements: { mode: "payment", amount: 2250, currency: "eur" },
      expressCheckout: { buttonHeight: 44 },
      mounted: ["#drawer-express-checkout"],
    });
    // The wallet is asked for a whole billing address, and for the phone
    // an order needs.
    const asked = await page.evaluate(
      () =>
        new Promise((resolve) =>
          (window as any).__stripe.on["expressCheckout:click"]({ resolve }),
        ),
    );
    expect(asked).toMatchObject({
      emailRequired: true,
      phoneNumberRequired: true,
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
          phone: "+33 6 12 34 56 78",
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
      phone: "+33 6 12 34 56 78",
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
      await expect(panel.locator("[data-drawer-total]")).toHaveText("8,00€");
      await panel
        .getByRole("button", { name: "Un de plus : Red Plate" })
        .click();
      await expect(noScript).toHaveURL(`${SHOP}/p/red-plate#cart`);
      await expect(panel.locator("[data-drawer-total]")).toHaveText(
        "16,00€",
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
    await expect(page.locator("[data-cart-subtotal]")).toHaveText("25,00€");
    await expect(
      page.locator("[data-cart-discount=TEN] [data-discount-amount]"),
    ).toHaveText("-2,50€");
    await expect(page.locator("main [data-cart-total]")).toHaveText(
      "22,50€",
    );

    // Charged the discounted total, and the order and its invoice say so.
    await page.getByRole("link", { name: "Commander" }).click();
    await expect(page.locator("[data-checkout-discount=TEN]")).toHaveText(
      "-2,50€",
    );
    await expect(page.locator("[data-checkout-total]")).toHaveText("22,50€");
    await placeOrder(page, SHOP, true);
    expect(stripe.lastIntent()!.amount).toBe(2250);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    await expect(page.locator("[data-order-discount=TEN]")).toContainText(
      "2,50€",
    );
    await expect(page.locator("[data-order-total]")).toHaveText("22,50€");
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
      "-1,25€",
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
      "-2.50 €",
    );
    await page.goto(`/t/${slug}/discounts`);
    await expect(
      page.locator("[data-discount=TEN] [data-discount-orders]"),
    ).toHaveText("1");
  } finally {
    await stripe.stop();
  }
});

test("an order keeps what brought the shopper, as far as they allowed it", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Source Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);
    await app.start(request, await mintKey(page, slug, true), 1);

    /// Orders what the cart holds; the order's page in the back office.
    const order = async (fill: boolean) => {
      await placeOrder(page, SHOP, fill);
      await confirmPayment(page, request, SHOP, stripe, webhookUrl);
      const reference =
        (await page.locator("[data-order-reference]").textContent())?.trim() ??
        "";
      const row = page.locator(`[data-order="${reference}"]`);
      await expect(async () => {
        await page.goto(`/t/${slug}/orders`);
        await expect(row).toHaveCount(1, { timeout: 1000 });
      }).toPass({ timeout: 15_000 });
      await row.getByRole("link", { name: reference }).click();
      await expect(page.locator("[data-order-total]")).toBeVisible();
    };

    // A newsletter's link brings the visitor, who accepts the cookies on
    // the page they landed on: the campaign is kept from there.
    const landing = `${SHOP}/p/blue-mug?utm_source=Lettre&utm_medium=email&utm_campaign=rentree`;
    await page.goto(landing);
    await page
      .locator("[data-consent]")
      .getByRole("button", { name: "Tout accepter" })
      .click();
    await expect(page).toHaveURL(landing);
    // Days later an ad brings them back, and they buy.
    await page.goto(`${SHOP}/p/blue-mug?gclid=Cj0KCQtest&page=1`);
    await addToCart(page);
    await order(true);
    const source = page.locator("[data-order-source]");
    await expect(source).toHaveAttribute("data-order-source", "google / cpc");
    const last = source.locator("[data-order-source-click=last]");
    await expect(last.locator("[data-order-source-channel]")).toHaveText(
      "google / cpc",
    );
    await expect(last).toContainText("Cj0KCQtest");
    await expect(last).toContainText("/p/blue-mug");
    const first = source.locator("[data-order-source-click=first]");
    await expect(first.locator("[data-order-source-channel]")).toHaveText(
      "lettre / email",
    );
    await expect(first).toContainText("rentree");

    // The answer taken back takes what was kept with it: the next order,
    // though an ad brought it, says nothing.
    await page.goto(`${SHOP}/cookies`);
    await page
      .locator("[data-cookies]")
      .getByRole("button", { name: "Tout refuser" })
      .click();
    await page.goto(`${SHOP}/p/blue-mug?gclid=Cj0KCQagain`);
    await addToCart(page);
    await order(false);
    await expect(page.locator("[data-order-source]")).toHaveCount(0);

    // Measuring allowed, advertising not: the campaign is kept, the click
    // id is not.
    await page.goto(`${SHOP}/cookies`);
    const choices = page.locator("[data-cookies]");
    await choices.getByRole("checkbox", { name: /Mesure d'audience/ }).check();
    await choices.getByRole("button", { name: "Enregistrer mes choix" }).click();
    await page.goto(
      `${SHOP}/p/blue-mug?gclid=Cj0KCQthird&utm_source=google&utm_medium=cpc`,
    );
    await addToCart(page);
    await order(false);
    await expect(source).toHaveAttribute("data-order-source", "google / cpc");
    await expect(source).not.toContainText("Cj0KCQthird");
    await expect(source.locator("[data-order-source-click=first]")).toHaveCount(0);
  } finally {
    await stripe.stop();
  }
});

test("a sale keeps what it cost and shows in the sales report, by product and by source", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Report Shop");
    await createTenant(page, name);
    await page.goto(`/t/${slug}/categories`);
    await page.getByLabel("Name").fill("Mugs");
    await page.getByRole("button", { name: "Create category" }).click();
    await expect(page.getByRole("status")).toContainText("Category created.");
    const mugUrl = await publishProduct(page, slug, "Blue Mug", "12.50", "Mugs");
    await publishProduct(page, slug, "Red Mug", "14.50", "Mugs");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);
    await app.start(request, await mintKey(page, slug, true), 2);

    // Nothing sold yet: the category lists the newest first.
    const listed = async () =>
      page.locator("ul.catalog > li[data-product]").evaluateAll((cards) =>
        cards.map((card) => card.getAttribute("data-product")),
      );
    await page.goto(`${SHOP}/c/mugs`);
    expect(await listed()).toEqual(["red-mug", "blue-mug"]);

    await page.goto(`/t/${slug}/orders`);
    await page.getByRole("link", { name: "Sales report", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${slug}/sales$`));
    await expect(page.locator("[data-sales-empty]")).toBeVisible();

    // The mug costs the shop 4.00.
    await page.goto(`${mugUrl}?tab=margin`);
    await page.getByLabel("Cost of one (EUR)").fill("4");
    await page.getByRole("button", { name: "Record what it costs" }).click();
    await expect(page.getByRole("status")).toContainText("Costs saved.");

    // A newsletter brings a shopper who allows the shop to keep it, and buys.
    await page.goto(`${SHOP}/p/blue-mug?utm_source=Lettre&utm_medium=email`);
    await page
      .locator("[data-consent]")
      .getByRole("button", { name: "Tout accepter" })
      .click();
    // The product's own button: the other mug, shown beside it, has one too.
    await page
      .locator("form")
      .filter({ hasText: "Quantité" })
      .getByRole("button", { name: "Ajouter au panier" })
      .click();
    await expect(page.locator("[data-cart-drawer].open .drawer-panel")).toBeVisible();
    await placeOrder(page, SHOP, true);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    const reference =
      (await page.locator("[data-order-reference]").textContent())?.trim() ?? "";
    const row = page.locator(`[data-order="${reference}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/orders`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: reference }).click();

    // The order says what it left: 12.50 less the 2.08 of VAT in it and
    // the mug's 4.00 (the shop told no other cost).
    const costs = page.locator("[data-order-costs]");
    await expect(costs).toContainText("4.00 €");
    await expect(costs.locator("[data-order-profit]")).toHaveAttribute("data-order-profit", "642");
    await expect(costs.locator("[data-order-profit]")).toHaveText("6.42 €");

    // And the report adds it up.
    await page.goto(`/t/${slug}/sales`);
    const totals = page.locator("[data-sales-totals=EUR]");
    await expect(totals.locator("[data-sales-orders]")).toHaveAttribute("data-sales-orders", "1");
    await expect(totals.locator("[data-sales-revenue]")).toHaveAttribute("data-sales-revenue", "1250");
    await expect(totals.locator("[data-sales-profit]")).toHaveAttribute("data-sales-profit", "642");
    const source = page.locator('[data-sales-source="lettre / email"]');
    await expect(source).toContainText("12.50 €");
    await expect(source).toContainText("6.42 €");
    const seller = page.locator('[data-sales-product="Blue Mug"]');
    await expect(seller.locator("[data-sales-quantity]")).toHaveText("1");
    await expect(seller).toContainText("6.42 €");
    await expect(page.locator('[data-sales-category="Mugs"]')).toContainText("12.50 €");

    // What sold comes first in its category, on the storefront: the
    // ranking follows the sale, the newest first again when asked.
    await expect(async () => {
      await page.goto(`${SHOP}/c/mugs`);
      expect(await listed()).toEqual(["blue-mug", "red-mug"]);
    }).toPass({ timeout: 20_000 });
    await expect(page.locator("[data-sort]")).toHaveValue("best_selling");
    await expect(page.locator('meta[name="robots"]')).toHaveCount(0);
    await page.goto(`${SHOP}/c/mugs?sort=newest`);
    expect(await listed()).toEqual(["red-mug", "blue-mug"]);
    await page.goto(`/t/${slug}/sales`);

    // The order as a file: one line for its one item, with what is the
    // order's own, and its invoice's number and VAT once it is issued.
    let csv = "";
    await expect(async () => {
      csv = await (await page.request.get(`/t/${slug}/orders/export`)).text();
      expect(csv).toMatch(/,F[^,]*-\d{6},2\.08,/);
    }).toPass({ timeout: 15_000 });
    const lines = csv.trimEnd().split("\r\n");
    expect(lines).toHaveLength(2);
    expect(lines[1].startsWith(`${reference},`)).toBe(true);
    expect(lines[0]).toContain("reference,placed_at,status");
    expect(lines[1]).toContain(",EUR,Blue Mug,,,1,12.50,12.50,standard,,0.00,,0.00,12.50,0.00,");
    expect(lines[1]).toContain(",lettre / email,lettre / email,");
    // A file of other days has none of it.
    const other = await page.request.get(`/t/${slug}/orders/export?from=2020-01-01&to=2020-01-31`);
    expect((await other.text()).trimEnd().split("\r\n")).toHaveLength(1);

    // A period before the shop sold anything is empty; the last week is not.
    await page.getByLabel("From", { exact: true }).fill("2020-01-01");
    await page.getByLabel("To", { exact: true }).fill("2020-01-31");
    await page.getByRole("button", { name: "Show these days" }).click();
    await expect(page).toHaveURL(/from=2020-01-01&to=2020-01-31/);
    await expect(page.locator("[data-sales-empty]")).toBeVisible();
    await page.getByLabel("Period").selectOption("7");
    await page.getByRole("button", { name: "Show", exact: true }).click();
    await expect(page).toHaveURL(/days=7/);
    await expect(totals.locator("[data-sales-orders]")).toHaveAttribute("data-sales-orders", "1");
  } finally {
    await stripe.stop();
  }
});

test("the shop's Google tag waits for consent, then counts the cart, the checkout and the sale once", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    // A stand-in for Google's script: the page's own calls are what is read.
    let loads = 0;
    await page.route("https://www.googletagmanager.com/**", (route) => {
      loads += 1;
      return route.fulfill({ contentType: "application/javascript", body: "" });
    });
    /// What the page told the tag, in order, without the consent calls.
    const told = async () =>
      (
        await page.evaluate(() =>
          ((window as unknown as { dataLayer?: unknown[][] }).dataLayer ?? []).map(
            (entry) => JSON.parse(JSON.stringify(Array.from(entry))),
          ),
        )
      ).filter((entry: unknown[]) => entry[0] !== "consent" && entry[0] !== "js");
    const events = async (name: string) =>
      (await told()).filter((entry: unknown[]) => entry[0] === "event" && entry[1] === name);
    const tag = page.locator("script[data-google-tag]");

    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Tag Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);

    // The merchant sets the tag up; an id that is not one is refused, and a
    // label needs the Google Ads id it belongs to.
    await page.goto(`/t/${slug}/google-tag`);
    const form = page.locator("[data-google-tag]");
    await expect(form).toHaveAttribute("data-google-tag", "none");
    await form.getByLabel("Analytics measurement ID").fill("UA-12345-1");
    await form.getByRole("button", { name: "Save the tag" }).click();
    await expect(page.getByRole("alert").first()).toContainText("An Analytics ID reads G-XXXXXXXXXX");
    await form.getByLabel("Analytics measurement ID").fill("g-e2etest123");
    await form.getByLabel("Label of the sale conversion").fill("SaLe_1");
    await form.getByRole("button", { name: "Save the tag" }).click();
    await expect(page.getByRole("alert").first()).toContainText("need the Google Ads conversion ID");
    await form.getByLabel("Analytics measurement ID").fill("g-e2etest123");
    await form.getByLabel("Google Ads conversion ID").fill("AW-123456789");
    await form.getByLabel("Label of the sale conversion").fill("AW-123456789/SaLe_1");
    await form.getByLabel("Label of the checkout conversion").fill("ChEcK");
    await form.getByRole("checkbox", { name: "Enhanced conversions" }).check();
    await form.getByRole("button", { name: "Save the tag" }).click();
    await expect(page.getByRole("status")).toContainText("Tag saved");
    await expect(form).toHaveAttribute("data-google-tag", "set");
    await expect(form.getByLabel("Analytics measurement ID")).toHaveValue("G-E2ETEST123");
    await expect(form.getByLabel("Label of the sale conversion")).toHaveValue("SaLe_1");
    await app.start(request, await mintKey(page, slug, true), 1);

    // Nothing of Google's loads before the visitor answers, nor after a
    // refusal.
    await page.goto(`${SHOP}/p/blue-mug`);
    await expect(page.locator("[data-consent]")).toBeVisible();
    await expect(tag).toHaveCount(0);
    expect(await told()).toEqual([]);
    await page
      .locator("[data-consent]")
      .getByRole("button", { name: "Tout refuser" })
      .click();
    await expect(page.locator("[data-consent]")).toHaveCount(0);
    await expect(tag).toHaveCount(0);
    await addToCart(page);
    expect(await told()).toEqual([]);
    expect(loads).toBe(0);

    // Accepted: the tag loads on every page, once the storefront has the
    // merchant's settings, configured for both products.
    await page.goto(`${SHOP}/cookies`);
    await page
      .locator("[data-cookies]")
      .getByRole("button", { name: "Tout accepter" })
      .click();
    await expect(async () => {
      await page.goto(`${SHOP}/p/blue-mug`);
      await expect(tag).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(tag).toHaveAttribute(
      "src",
      "https://www.googletagmanager.com/gtag/js?id=G-E2ETEST123",
    );
    expect(loads).toBeGreaterThan(0);
    expect(await told()).toEqual([
      ["config", "G-E2ETEST123"],
      ["config", "AW-123456789", { allow_enhanced_conversions: true }],
    ]);

    // An item added from the product's page, which stays where it is: told
    // to Analytics, and to no conversion, since none has a label for it.
    await addToCart(page);
    const item = {
      item_id: expect.any(String),
      item_name: "Blue Mug",
      price: 12.5,
      quantity: 1,
    };
    await expect.poll(() => events("add_to_cart")).toEqual([
      [
        "event",
        "add_to_cart",
        { currency: "EUR", value: 12.5, items: [item], send_to: "G-E2ETEST123" },
      ],
    ]);
    expect(await events("conversion")).toEqual([]);
    // The next page does not tell it again.
    await page.goto(`${SHOP}/boutique`);
    expect(await events("add_to_cart")).toEqual([]);
    // Added from a listing's card, which loads the page again: told there.
    await page
      .locator("[data-product=blue-mug]")
      .getByRole("button", { name: "Ajouter au panier" })
      .click();
    await expect(page.locator("[data-cart-drawer].open .drawer-panel")).toBeVisible();
    await expect.poll(async () => (await events("add_to_cart")).length).toBe(1);

    // The checkout opening is a secondary conversion, with its label; the
    // cart holds three mugs, the one added before the tag loaded too.
    await page.goto(`${SHOP}/checkout`);
    expect(await events("begin_checkout")).toEqual([
      [
        "event",
        "begin_checkout",
        {
          currency: "EUR",
          value: 37.5,
          items: [{ ...item, quantity: 3 }],
          send_to: "G-E2ETEST123",
        },
      ],
    ]);
    expect(await events("conversion")).toEqual([
      ["event", "conversion", { send_to: "AW-123456789/ChEcK", value: 37.5, currency: "EUR" }],
    ]);

    // The sale, on the order's page: the buyer's details hashed first, then
    // the purchase with the order as its transaction, and the conversion.
    await placeOrder(page, SHOP, true);
    const orderUrl = await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    const orderId = orderUrl.split("/").pop()!;
    const sha256 = (text: string) => createHash("sha256").update(text).digest("hex");
    const sale = await told();
    expect(sale).toEqual([
      ["config", "G-E2ETEST123"],
      ["config", "AW-123456789", { allow_enhanced_conversions: true }],
      [
        "set",
        "user_data",
        {
          sha256_email_address: sha256("ada@example.org"),
          sha256_phone_number: expect.stringMatching(/^[0-9a-f]{64}$/),
          address: {
            sha256_first_name: sha256("ada"),
            sha256_last_name: sha256("lovelace"),
            street: "1 rue de la Paix",
            city: "Paris",
            postal_code: expect.any(String),
            country: "FR",
          },
        },
      ],
      [
        "event",
        "purchase",
        {
          transaction_id: orderId,
          value: 37.5,
          currency: "EUR",
          items: [{ ...item, quantity: 3 }],
          send_to: "G-E2ETEST123",
        },
      ],
      [
        "event",
        "conversion",
        { send_to: "AW-123456789/SaLe_1", value: 37.5, currency: "EUR", transaction_id: orderId },
      ],
    ]);
    // Once per order: the page looked at again counts nothing.
    await page.reload();
    await expect(tag).toHaveCount(1);
    expect(await events("purchase")).toEqual([]);
    expect(await events("conversion")).toEqual([]);

    // The merchant takes the tag off: the storefront follows.
    await page.goto(`/t/${slug}/google-tag`);
    for (const label of [
      "Analytics measurement ID",
      "Google Ads conversion ID",
      "Label of the sale conversion",
      "Label of the checkout conversion",
    ]) {
      await form.getByLabel(label).fill("");
    }
    await form.getByRole("checkbox", { name: "Enhanced conversions" }).uncheck();
    await form.getByRole("button", { name: "Save the tag" }).click();
    await expect(page.getByRole("status")).toContainText("No tag");
    await expect(async () => {
      await page.goto(`${SHOP}/boutique`);
      await expect(tag).toHaveCount(0, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
  } finally {
    await stripe.stop();
  }
});

test("the shop's Meta pixel waits for advertising to be allowed, and a sale is told from the server too", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  const meta = new FakeMeta();
  await meta.start();
  try {
    // A stand-in for Meta's script: the page's own calls are what is read.
    let loads = 0;
    await page.route("https://connect.facebook.net/**", (route) => {
      loads += 1;
      return route.fulfill({ contentType: "application/javascript", body: "" });
    });
    /// What the page told the pixel, in order.
    const told = async (): Promise<unknown[][]> =>
      page.evaluate(() =>
        Array.from(
          (window as unknown as { fbq?: { queue: ArrayLike<unknown>[] } }).fbq?.queue ?? [],
        ).map((entry) => JSON.parse(JSON.stringify(Array.from(entry)))),
      );
    const events = async (name: string) =>
      (await told()).filter((entry) => entry[0] === "track" && entry[1] === name);
    const pixel = page.locator("script[data-meta-pixel]");
    const PIXEL = "123456789012345";

    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Pixel Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);

    // The merchant sets the pixel and its token; an id that is not digits
    // and a token that is not one are refused, and the token is never shown.
    await page.goto(`/t/${slug}/meta-pixel`);
    const form = page.locator("[data-meta-pixel]");
    await expect(form).toHaveAttribute("data-meta-pixel", "none");
    await form.getByLabel("Pixel ID").fill("12ab");
    await form.getByRole("button", { name: "Save the pixel" }).click();
    await expect(page.getByRole("alert").first()).toContainText("5 to 20 digits");
    await form.getByLabel("Pixel ID").fill(PIXEL);
    await form.getByLabel("Conversions API token").fill("not a token!");
    await form.getByRole("button", { name: "Save the pixel" }).click();
    await expect(page.getByRole("alert").first()).toContainText("does not look like");
    await expect(form).toHaveAttribute("data-meta-pixel", "none");
    await form.getByLabel("Pixel ID").fill(PIXEL);
    await form.getByLabel("Conversions API token").fill("EAAe2eTOKEN");
    await form.getByRole("button", { name: "Save the pixel" }).click();
    await expect(page.getByRole("status")).toContainText("Pixel saved");
    await expect(form).toHaveAttribute("data-meta-pixel", "token");
    await expect(form.getByLabel("Pixel ID")).toHaveValue(PIXEL);
    await expect(form.getByLabel("Conversions API token")).toHaveValue("");
    await app.start(request, await mintKey(page, slug, true), 1);

    // Nothing of Meta's is written before the visitor answers, nor for one
    // who allows measuring only.
    await page.goto(`${SHOP}/p/blue-mug`);
    await expect(page.locator("[data-consent]")).toBeVisible();
    await expect(pixel).toHaveCount(0);
    expect(await told()).toEqual([]);
    await page.goto(`${SHOP}/cookies`);
    const choices = page.locator("[data-cookies]");
    await choices.getByRole("checkbox", { name: /Mesure d'audience/ }).check();
    await choices.getByRole("button", { name: "Enregistrer mes choix" }).click();
    await page.goto(`${SHOP}/p/blue-mug`);
    await expect(page.locator("[data-consent]")).toHaveCount(0);
    await expect(pixel).toHaveCount(0);
    expect(await told()).toEqual([]);
    expect(loads).toBe(0);

    // Advertising allowed: the pixel starts on every page, once the
    // storefront has the merchant's settings, and hears of the page and of
    // the product it shows.
    await page.goto(`${SHOP}/cookies`);
    await choices.getByRole("button", { name: "Tout accepter" }).click();
    await expect(async () => {
      await page.goto(`${SHOP}/p/blue-mug`);
      await expect(pixel).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    expect(loads).toBeGreaterThan(0);
    const id = { eventID: expect.stringMatching(/^[0-9a-f]{32}$/) };
    const mug = {
      content_type: "product",
      content_ids: [expect.any(String)],
      contents: [{ id: expect.any(String), quantity: 1, item_price: 12.5 }],
    };
    expect(await told()).toEqual([
      ["init", PIXEL],
      ["track", "PageView", {}, id],
      [
        "track",
        "ViewContent",
        { ...mug, content_name: "Blue Mug", value: 12.5, currency: "EUR" },
        id,
      ],
    ]);

    // An item added from the product's page, which stays where it is: told
    // once, and the page the cart's script fetched is not counted as seen.
    await addToCart(page);
    await expect
      .poll(() => events("AddToCart"))
      .toEqual([["track", "AddToCart", { ...mug, value: 12.5, currency: "EUR" }, id]]);
    expect(await events("PageView")).toHaveLength(1);
    expect(await events("ViewContent")).toHaveLength(1);
    // The next page does not tell it again.
    await page.goto(`${SHOP}/boutique`);
    expect(await events("AddToCart")).toEqual([]);
    expect(await events("PageView")).toHaveLength(1);

    await page.goto(`${SHOP}/checkout`);
    expect(await events("InitiateCheckout")).toEqual([
      ["track", "InitiateCheckout", { ...mug, num_items: 1, value: 12.5, currency: "EUR" }, id],
    ]);

    // The sale, on the order's page, under the order's id; nothing was
    // sent from the server before it.
    expect(meta.received).toEqual([]);
    await placeOrder(page, SHOP, true);
    const orderUrl = await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    const orderId = orderUrl.split("/").pop()!;
    const reference =
      (await page.locator("[data-order-reference]").textContent())?.trim() ?? "";
    expect(await events("Purchase")).toEqual([
      [
        "track",
        "Purchase",
        { ...mug, num_items: 1, value: 12.5, currency: "EUR", order_id: reference },
        { eventID: orderId },
      ],
    ]);
    // The dashboard tells Meta the same sale, under the same id, with the
    // buyer hashed and the browser the checkout was made in.
    const sha256 = (text: string) => createHash("sha256").update(text).digest("hex");
    await expect.poll(() => meta.received.length, { timeout: 15_000 }).toBe(1);
    const sent = meta.received[0];
    expect(sent.pixelId).toBe(PIXEL);
    expect(sent.token).toBe("EAAe2eTOKEN");
    expect(sent.event).toMatchObject({
      event_name: "Purchase",
      event_id: orderId,
      action_source: "website",
      user_data: {
        em: [sha256("ada@example.org")],
        fn: [sha256("ada")],
        ln: [sha256("lovelace")],
        ct: [sha256("paris")],
        country: [sha256("fr")],
        client_user_agent: await page.evaluate(() => navigator.userAgent),
      },
      custom_data: { currency: "EUR", value: 12.5, order_id: reference, num_items: 1 },
    });
    expect(sent.event.event_source_url).toContain("/checkout");
    // Once per order: the page looked at again counts nothing.
    await page.reload();
    await expect(pixel).toHaveCount(1);
    expect(await events("Purchase")).toEqual([]);

    // A token Meta refuses: the next sale leaves its reason beside the
    // token, for the merchant to read.
    await page.goto(`/t/${slug}/meta-pixel`);
    await expect(page.locator("[data-meta-refusal]")).toHaveCount(0);
    await form.getByLabel("Conversions API token").fill("EAArefused");
    await form.getByRole("button", { name: "Save the pixel" }).click();
    await expect(page.getByRole("status")).toContainText("Pixel saved");
    await page.goto(`${SHOP}/p/blue-mug`);
    await addToCart(page);
    await placeOrder(page, SHOP, false);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    await expect(async () => {
      await page.goto(`/t/${slug}/meta-pixel`);
      await expect(page.locator("[data-meta-refusal]")).toContainText(
        "Invalid OAuth access token",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
    expect(meta.received).toHaveLength(1);

    // The merchant takes the token off, then the pixel: the storefront
    // follows.
    await form.getByRole("checkbox", { name: "Take the token off" }).check();
    await form.getByRole("button", { name: "Save the pixel" }).click();
    await expect(form).toHaveAttribute("data-meta-pixel", "pixel");
    await expect(page.locator("[data-meta-refusal]")).toHaveCount(0);
    await form.getByLabel("Pixel ID").fill("");
    await form.getByRole("button", { name: "Save the pixel" }).click();
    await expect(page.getByRole("status")).toContainText("No pixel");
    await expect(async () => {
      await page.goto(`${SHOP}/boutique`);
      await expect(pixel).toHaveCount(0, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
  } finally {
    await meta.stop();
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
    // Thirty days to send goods back, at the shop's cost.
    await page.getByLabel("Return window, in days").fill("30");
    await page
      .getByRole("checkbox", { name: "The shop pays for sending goods back" })
      .check();
    await page.getByRole("button", { name: "Set return terms" }).click();
    await expect(page.getByRole("status")).toContainText("Return terms set.");
    await expect(page.locator("[data-return-terms]")).toHaveAttribute(
      "data-return-terms",
      "30",
    );
    await app.start(request, await mintKey(page, slug, true), 1);

    // The product's page tells search engines what delivery costs, where
    // to, and how returns go, once the storefront has the shop's terms.
    await expect(async () => {
      await page.goto(`${SHOP}/p/blue-mug`);
      const [product] = (
        await page.locator('script[type="application/ld+json"]').allTextContents()
      ).map((text) => JSON.parse(text));
      expect(product.offers.shippingDetails).toEqual([
        {
          "@type": "OfferShippingDetails",
          shippingRate: { "@type": "MonetaryAmount", value: "4.90", currency: "EUR" },
          shippingDestination: [
            { "@type": "DefinedRegion", addressCountry: "BE" },
            { "@type": "DefinedRegion", addressCountry: "FR" },
          ],
        },
        {
          "@type": "OfferShippingDetails",
          shippingRate: { "@type": "MonetaryAmount", value: "6.90", currency: "EUR" },
          shippingDestination: [{ "@type": "DefinedRegion", addressCountry: "FR" }],
        },
      ]);
      expect(product.offers.hasMerchantReturnPolicy).toEqual({
        "@type": "MerchantReturnPolicy",
        applicableCountry: ["BE", "FR"],
        returnPolicyCategory: "https://schema.org/MerchantReturnFiniteReturnWindow",
        merchantReturnDays: 30,
        returnMethod: "https://schema.org/ReturnByMail",
        returnFees: "https://schema.org/FreeReturn",
      });
    }).toPass({ timeout: 15_000 });

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
      "4,90€",
    );
    await expect(page.locator("[data-checkout-total]")).toHaveText("29,90€");
    await expect(page.locator("[data-checkout-vat]")).toContainText(
      "VAT FR 20%",
    );
    expect(
      await page.evaluate(() => (window as any).__stripe.elements.amount),
    ).toBe(2990);

    // VIP costs more; Belgium has only the tracked parcel; the US is not offered.
    await vip.check();
    await expect(page.locator("[data-checkout-shipping]")).toHaveText(
      "6,90€",
    );
    await expect(page.locator("[data-checkout-total]")).toHaveText("31,90€");
    expect(
      await page.evaluate(() => (window as any).__stripe.elements.amount),
    ).toBe(3190);
    await page.getByLabel("Pays", { exact: true }).selectOption("BE");
    await expect(vip).toBeHidden();
    await expect(tracked).toBeChecked();
    await expect(page.locator("[data-checkout-total]")).toHaveText("29,90€");
    await expect(
      page.getByLabel("Pays", { exact: true }).locator("option"),
    ).toHaveText(["Belgique", "France"]);

    // Back to France with VIP: charged on top, on the order and its invoice.
    await fillCheckout(page, true);
    await vip.check();
    await expect(page.locator("[data-checkout-total]")).toHaveText("31,90€");
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    expect(stripe.lastIntent()!.amount).toBe(3190);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    await expect(page.locator("[data-order-shipping=VIP]")).toContainText(
      "6,90€",
    );
    await expect(page.locator("[data-order-total]")).toHaveText("31,90€");
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
      "6.90 €",
    );
  } finally {
    await stripe.stop();
  }
});

test("a shopper sees how long delivery takes, the product's own or the shop's default", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Delay Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    const lamp = await publishProduct(page, slug, "Floor Lamp", "30.00");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);

    // The lamp takes 8 to 15 days; everything else the shop's 2 to 4.
    await page.goto(lamp);
    await page.getByLabel("Shortest delivery, in days").fill("8");
    await page.getByLabel("Longest delivery, in days").fill("15");
    await page.getByRole("button", { name: "Set delivery time" }).click();
    await expect(page.getByRole("status")).toContainText("Delivery time set.");
    await page.goto(`/t/${slug}/shipping`);
    await page.getByLabel("Shortest delivery, in days").fill("4");
    await page.getByLabel("Longest delivery, in days").fill("2");
    await page.getByRole("button", { name: "Set default delivery time" }).click();
    await expect(page.getByRole("alert")).toContainText("the shortest first");
    await page.getByLabel("Shortest delivery, in days").fill("2");
    await page.getByLabel("Longest delivery, in days").fill("4");
    await page.getByRole("button", { name: "Set default delivery time" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Default delivery time set.",
    );
    await app.start(request, await mintKey(page, slug, true), 2);

    // The default is the last change: the shop's copy shows it a moment
    // after the products.
    await expect(async () => {
      await page.goto(`${SHOP}/p/blue-mug`);
      await expect(page.locator("[data-delivery-days]")).toHaveText(
        "Livraison avec suivi en 2 à 4 jours",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
    await addToCart(page);
    await page.goto(`${SHOP}/p/floor-lamp`);
    await expect(page.locator("[data-delivery-days]")).toHaveText(
      "Livraison avec suivi en 8 à 15 jours",
    );
    await addToCart(page);

    // The cart says each line's time, and the order's: the longest.
    await page.goto(`${SHOP}/cart`);
    await expect(
      page.locator("[data-cart-line='Floor Lamp'] [data-line-delivery]"),
    ).toHaveText("Livraison en 8 à 15 jours");
    await expect(
      page.locator("[data-cart-line='Blue Mug'] [data-line-delivery]"),
    ).toHaveText("Livraison en 2 à 4 jours");
    await expect(page.locator("[data-cart-delivery]")).toContainText(
      "8 à 15 jours",
    );
    await page.getByRole("link", { name: "Commander" }).click();
    await expect(page.locator("[data-checkout-delivery]")).toHaveText(
      "8 à 15 jours",
    );

    // The order keeps the time it was sold with.
    await fillCheckout(page, true);
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    await expect(page.locator("[data-order-delivery]")).toContainText(
      "8 à 15 jours",
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
      .selectOption({ label: "GPS Tracker (30.00 €)" });
    await page.getByLabel("Offer price").fill("30.00");
    await page.getByRole("button", { name: "Create offer" }).click();
    await expect(page.getByRole("alert").first()).toContainText(
      "below the product's price",
    );

    // One tracker at 12.00 while the cart holds a collar.
    await page
      .getByLabel("Item")
      .selectOption({ label: "GPS Tracker (30.00 €)" });
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
    await expect(row.locator("[data-offer-price]")).toHaveText("12.00 €");
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
    await expect(card.locator("[data-offer-price]")).toHaveText("12,00€");
    await expect(page.locator("[data-checkout-total]")).toHaveText("20,00€");
    await card
      .getByRole("button", { name: "Ajouter GPS Tracker à 12,00€" })
      .click();
    await expect(page).toHaveURL(`${SHOP}/checkout`);
    await expect(page.locator("[data-checkout-offers]")).toHaveCount(0);
    const line = page.locator('[data-summary-line="GPS Tracker"]');
    await expect(line.locator("[data-line-offer]")).toContainText(
      "1 à 12,00€",
    );
    await expect(page.locator("[data-checkout-subtotal]")).toHaveText(
      "32,00€",
    );
    await expect(page.locator("[data-checkout-total]")).toHaveText("32,00€");

    // A second tracker is at full price.
    await line
      .getByRole("button", { name: "Un de plus : GPS Tracker" })
      .click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("62,00€");
    await line
      .getByRole("button", { name: "Un de moins : GPS Tracker" })
      .click();
    await expect(page.locator("[data-checkout-total]")).toHaveText("32,00€");

    // Charged what the page showed; the order says what the tracker cost.
    await fillCheckout(page, true);
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    expect(stripe.lastIntent()!.amount).toBe(3200);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    await expect(page.locator("[data-order-total]")).toHaveText("32,00€");

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
