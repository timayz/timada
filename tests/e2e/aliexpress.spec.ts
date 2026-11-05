import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { FakeAliexpress, PRODUCT_ID } from "./aliexpress";
import { registerWithEmail, uniqueEmail } from "./email";
import { ExampleApp, mintKey } from "./storefront";
import { FakeStripe, fakeStripeJs, stripeSignature } from "./stripe";
import { createTenant, setLegalIdentity, uniqueTenant } from "./tenants";

// AliExpress end to end against a stand-in (aliexpress.ts): the platform's
// app (config/e2e.toml) acts for the buyer account a company connects on its
// page; a merchant then chooses what the shop buys in, imports a product by
// its address (a draft with its options, pictures, stock and prices by the
// shop's rule), then reads it again: stock and price follow AliExpress, and a
// product AliExpress stops selling is taken off sale.

/// The rule a tenant is created with, as the page reads it.
const USUAL_RATES =
  "Up to 10.00 €: cost ×3.00, up to .90 · Up to 25.00 €: cost ×2.50, up to .90 · Up to 50.00 €: cost ×2.20, up to .90 · Up to 100.00 €: cost ×1.80, up to .90 · Up to 250.00 €: cost ×1.50, up to .90 · above: cost ×1.30, up to .90";

/// One rule for every cost in place of the rates a tenant is created with:
/// their rows left blank are dropped.
async function oneRule(page: Page, multiplier: string, rounding: string) {
  for (const band of [1, 2, 3, 4, 5]) {
    await page.getByLabel(`Band ${band}: costs up to`).fill("");
    await page.getByLabel(`Band ${band}: multiplier`).fill("");
  }
  await page.getByLabel("Multiply the cost by").fill(multiplier);
  await page.getByLabel("Round", { exact: true }).selectOption(rounding);
}

test("a merchant imports an AliExpress product and keeps it in step", async ({ page }) => {
  const aliexpress = new FakeAliexpress();
  await aliexpress.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Ali Shop");
    await createTenant(page, name);

    await page.getByRole("link", { name: "AliExpress", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${slug}/aliexpress$`));
    await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "none");
    await expect(page.locator("[data-aliexpress-company]")).toContainText("Not linked");
    await expect(page.locator("[data-aliexpress-rule]")).toHaveText(USUAL_RATES);
    await expect(page.getByLabel("AliExpress product")).toHaveCount(0);

    // The shop sells as a company; its owner connects the company's buyer
    // account, which says yes on AliExpress.
    const company = await setLegalIdentity(page, slug);
    await page.goto(company.companyUrl);
    await expect(page.locator("[data-company-aliexpress]")).toHaveAttribute("data-company-aliexpress", "none");
    await page.getByRole("button", { name: "Connect the AliExpress account" }).click();
    await expect(page.getByRole("status")).toContainText("AliExpress account connected.");
    await expect(page.locator("[data-company-aliexpress]")).toHaveAttribute("data-company-aliexpress", "connected");
    await expect(page.locator("[data-aliexpress-account]")).toHaveText("shop@example.org");

    // The shop's market: a rule that is not one is refused; then saved.
    await page.goto(`/t/${slug}/aliexpress`);
    await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "none");
    await expect(page.locator("[data-aliexpress-account]")).toHaveText("shop@example.org");
    await page.getByLabel("Ships to (country)").fill("FR");
    await page.getByLabel("Worded in (language)").fill("fr");
    await page.getByLabel("Multiply the cost by").fill("0.5");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page).toHaveURL(/error=invalid_pricing_rule/);

    // The form shows the rates the tenant was created with; saved as they
    // are, they stay the shop's rule.
    await expect(page.getByLabel("Band 1: costs up to")).toHaveValue("10.00");
    await expect(page.getByLabel("Band 1: multiplier")).toHaveValue("3.00");
    await expect(page.getByLabel("Band 1: rounding")).toHaveValue("90");
    await expect(page.getByLabel("Band 5: costs up to")).toHaveValue("250.00");
    await expect(page.getByLabel("Band 5: multiplier")).toHaveValue("1.50");
    await expect(page.getByLabel("Multiply the cost by")).toHaveValue("1.30");
    await expect(page.getByLabel("Round", { exact: true })).toHaveValue("90");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("AliExpress settings saved.");
    await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "connected");
    await expect(page.locator("[data-aliexpress-rule]")).toHaveText(USUAL_RATES);

    // One rule for every cost, in their place.
    await oneRule(page, "2", "99");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status")).toContainText("AliExpress settings saved.");
    await expect(page.locator("[data-aliexpress-rule]")).toHaveText("Cost ×2.00, up to .99");

    // Cheap items take a higher rate: a band up to 5.00 at ×3, the rest as
    // before. A band without a rule is refused; one ending where another
    // does too.
    await page.getByLabel("New band: costs up to").fill("5");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page).toHaveURL(/error=invalid_pricing_rule/);
    await page.getByLabel("New band: costs up to").fill("5");
    await page.getByLabel("New band: multiplier").fill("3");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status")).toContainText("AliExpress settings saved.");
    await expect(page.locator("[data-aliexpress-rule]")).toHaveText(
      "Up to 5.00 €: cost ×3.00 · above: cost ×2.00, up to .99",
    );
    await expect(page.getByLabel("Band 1: costs up to")).toHaveValue("5.00");
    await expect(page.getByLabel("Band 1: multiplier")).toHaveValue("3.00");
    await expect(page.getByLabel("New band: costs up to")).toHaveValue("");
    await page.getByLabel("New band: costs up to").fill("5");
    await page.getByLabel("New band: multiplier").fill("4");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page).toHaveURL(/error=invalid_pricing_rule/);
    await expect(page.locator("[data-aliexpress-rule]")).toHaveText(
      "Up to 5.00 €: cost ×3.00 · above: cost ×2.00, up to .99",
    );

    // An address that is not an item's is refused; an item's is imported.
    await page.getByLabel("AliExpress product").fill("https://example.com/p/12");
    await page.getByRole("button", { name: "Import", exact: true }).click();
    await expect(page).toHaveURL(/error=invalid_aliexpress_product/);
    await page.getByLabel("AliExpress product").fill(`https://fr.aliexpress.com/item/${PRODUCT_ID}.html?spm=e2e`);
    await page.getByRole("button", { name: "Import", exact: true }).click();
    await expect(page.getByRole("status")).toContainText("Import started");
    const row = page.locator(`[data-aliexpress-link="${PRODUCT_ID}"]`);
    await expect(async () => {
      await page.reload();
      await expect(row).toHaveAttribute("data-link-status", "linked", { timeout: 1000 });
    }).toPass({ timeout: 20_000 });

    // A draft with both colours, their pictures and stock, priced by the
    // rule: rose costs 4.31, in the band; blue 5.50, above it, holds the 15.00
    // the band ends at, up to .99.
    await row.getByRole("link", { name: "Open" }).click();
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Cat Tunnel Deluxe");
    const productUrl = page.url();
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "draft");
    await expect(page.locator("[data-image]")).toHaveCount(4);
    const rose = page.locator("[data-variant]", { hasText: "Colour: Rose" });
    const blue = page.locator("[data-variant]", { hasText: "Colour: Blue" });
    await expect(rose.locator("[data-stock]")).toHaveAttribute("data-stock", "120");
    await expect(blue.locator("[data-stock]")).toHaveAttribute("data-stock", "0");
    await expect(rose.getByLabel("Price of Colour: Rose")).toHaveValue("12.93");
    await expect(blue.getByLabel("Price of Colour: Blue")).toHaveValue("15.99");
    await expect(page.locator("[data-product-aliexpress]")).toHaveAttribute("data-product-aliexpress", PRODUCT_ID);
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");

    // AliExpress has fewer roses, dearer: the next read follows.
    aliexpress.sku("12000037001").stock = 7;
    aliexpress.sku("12000037001").price = "6.00";
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.getByRole("status")).toContainText("Read again from AliExpress.");
    await expect(rose.locator("[data-stock]")).toHaveAttribute("data-stock", "7");
    await expect(rose.getByLabel("Price of Colour: Rose")).toHaveValue("15.99");

    // Its own rule, from the product page, applies at the next read.
    await page.getByLabel("Its own rule: multiply the cost by").fill("3");
    await page.getByRole("button", { name: "Save rule" }).click();
    await expect(page.getByRole("status")).toContainText("Price rule saved");
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(rose.getByLabel("Price of Colour: Rose")).toHaveValue("18.00");

    // AliExpress stops selling it: nothing left, and off sale.
    aliexpress.offline = true;
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.getByRole("status")).toContainText("taken off sale");
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "draft");
    await expect(rose.locator("[data-stock]")).toHaveAttribute("data-stock", "0");
    await expect(page.locator("[data-product-aliexpress]")).toHaveAttribute("data-aliexpress-offline");

    // Importing it again is refused; every call was signed with the secret.
    await page.goto(`/t/${slug}/aliexpress`);
    await page.getByLabel("AliExpress product").fill(PRODUCT_ID);
    await page.getByRole("button", { name: "Import", exact: true }).click();
    await expect(page).toHaveURL(/error=aliexpress_already_linked/);

    // Archived, the product is no longer kept in step, and the item can be
    // imported again: a fresh draft.
    await page.goto(productUrl);
    await page.getByRole("button", { name: "Archive" }).click();
    await expect(page.getByRole("status")).toContainText("Product archived.");
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-product-aliexpress]")).toHaveCount(0, { timeout: 1000 });
    }).toPass({ timeout: 10_000 });
    await page.goto(`/t/${slug}/aliexpress`);
    await expect(async () => {
      await page.reload();
      await expect(row).toHaveAttribute("data-link-status", "unlinked", { timeout: 1000 });
    }).toPass({ timeout: 10_000 });
    aliexpress.offline = false;
    await page.getByLabel("AliExpress product").fill(PRODUCT_ID);
    await page.getByRole("button", { name: "Import", exact: true }).click();
    await expect(page.getByRole("status")).toContainText("Import started");
    await expect(async () => {
      await page.reload();
      await expect(row).toHaveAttribute("data-link-status", "linked", { timeout: 1000 });
    }).toPass({ timeout: 20_000 });
    await row.getByRole("link", { name: "Open" }).click();
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Cat Tunnel Deluxe");
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "draft");
    expect(page.url()).not.toBe(productUrl);
    expect(aliexpress.badSignatures).toBe(0);
    expect(aliexpress.calls).toContain("/auth/token/create");
    expect(aliexpress.calls.filter((call) => call === "aliexpress.ds.product.get").length).toBeGreaterThanOrEqual(4);
  } finally {
    await aliexpress.stop();
  }
});

test("a merchant adds the combinations AliExpress gained as variants, or ignores them", async ({ page }) => {
  const aliexpress = new FakeAliexpress();
  await aliexpress.start();
  try {
    const productUrl = await importedShop(page, aliexpress);
    await expect(page.locator("[data-variant]")).toHaveCount(2);
    await expect(page.locator("[data-aliexpress-found]")).toHaveCount(0);

    // AliExpress now sells green and grey too: the next read offers them.
    aliexpress.skus.push({ id: "12000037003", colour: "Green", price: "3.00", stock: 9 });
    aliexpress.skus.push({ id: "12000037004", colour: "Grey", price: "3.50", stock: 4 });
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.getByRole("status")).toContainText("Read again from AliExpress.");
    const green = page.locator('[data-aliexpress-new="12000037003"]');
    const grey = page.locator('[data-aliexpress-new="12000037004"]');
    await expect(green).toContainText("Colour: Green");
    await expect(grey).toContainText("Colour: Grey");
    await expect(page.getByRole("button", { name: "Add all 2" })).toBeVisible();

    // The shop's list says there is something new.
    const slug = new URL(productUrl).pathname.split("/")[2];
    await expect(async () => {
      await page.goto(`/t/${slug}/aliexpress`);
      await expect(page.locator(`[data-aliexpress-link="${PRODUCT_ID}"] [data-link-new]`)).toHaveAttribute(
        "data-link-new",
        "2",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 10_000 });
    await page.goto(productUrl);

    // Grey is ignored, then offered again; green is added.
    await page.getByRole("button", { name: "Ignore Colour: Grey" }).click();
    await expect(page.getByRole("status")).toContainText("No longer offered.");
    await expect(grey).toHaveCount(0);
    await expect(page.locator('[data-aliexpress-ignored-sku="12000037004"]')).toHaveCount(1);
    await page.locator("[data-aliexpress-ignored] summary").click();
    await page.getByRole("button", { name: "Offer Colour: Grey again" }).click();
    await expect(page.getByRole("status")).toContainText("Offered again.");
    await expect(grey).toHaveCount(1);

    await page.getByRole("button", { name: "Add Colour: Green as a variant" }).click();
    await expect(page.getByRole("status")).toContainText("1 combination added as a variant.");
    const variant = page.locator("[data-variant]", { hasText: "Colour: Green" });
    await expect(variant.locator("[data-stock]")).toHaveAttribute("data-stock", "9");
    await expect(variant.getByLabel("Price of Colour: Green")).toHaveValue("6.00");
    await expect(green).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Add all 2" })).toHaveCount(0);

    // The next read counts green like the others.
    aliexpress.sku("12000037003").stock = 3;
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.getByRole("status")).toContainText("Read again from AliExpress.");
    await expect(variant.locator("[data-stock]")).toHaveAttribute("data-stock", "3");
    await expect(page.locator("[data-variant]")).toHaveCount(3);
  } finally {
    await aliexpress.stop();
  }
});

/// A shop selling as a company whose account is connected to the stand-in,
/// with its market set; its slug.
async function connectedShop(page: Page, tenant: string): Promise<string> {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant(tenant);
  await createTenant(page, name);
  const company = await setLegalIdentity(page, slug);
  await page.goto(company.companyUrl);
  await page.getByRole("button", { name: "Connect the AliExpress account" }).click();
  await expect(page.locator("[data-company-aliexpress]")).toHaveAttribute("data-company-aliexpress", "connected");
  await page.goto(`/t/${slug}/aliexpress`);
  await oneRule(page, "2", "cent");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("AliExpress settings saved.");
  await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "connected");
  return slug;
}

test("two shops of one company buy through the same AliExpress account", async ({ page }) => {
  const aliexpress = new FakeAliexpress();
  await aliexpress.start();
  try {
    const slug = await connectedShop(page, "Ali Main Shop");
    const connections = aliexpress.calls.filter((call) => call === "/auth/token/create").length;
    const pro = uniqueTenant("Ali Pro Shop");
    await createTenant(page, pro.name);

    // Linked to the same company, the second shop is connected as soon as
    // its market is set: nobody says yes on AliExpress again.
    await page.goto(`/t/${slug}/invoicing`);
    const companyUrl = (await page.locator("[data-linked-company] a").getAttribute("href")) ?? "";
    await expect(async () => {
      await page.goto(`/t/${pro.slug}/invoicing`);
      await expect(page.getByRole("button", { name: "Link" })).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await page.getByLabel("Series code (blank for the company's first tenant)").fill("pro");
    await page.getByRole("button", { name: "Link" }).click();
    await expect(page.getByRole("status")).toContainText("Linked to the company.");
    await page.goto(`/t/${pro.slug}/aliexpress`);
    await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "none");
    await expect(page.locator("[data-aliexpress-account]")).toHaveText("shop@example.org");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "connected");
    await expect(page.getByLabel("AliExpress product")).toBeVisible();
    expect(aliexpress.calls.filter((call) => call === "/auth/token/create").length).toBe(connections);

    // Disconnected on the company's page, both shops lose it.
    await page.goto(companyUrl);
    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(page.getByRole("status")).toContainText("AliExpress account disconnected.");
    await expect(page.locator("[data-company-aliexpress]")).toHaveAttribute("data-company-aliexpress", "none");
    for (const shop of [slug, pro.slug]) {
      await page.goto(`/t/${shop}/aliexpress`);
      await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "configured");
      await expect(page.getByLabel("AliExpress product")).toHaveCount(0);
    }
  } finally {
    await aliexpress.stop();
  }
});

/// A shop connected to the stand-in with one product imported from it; the
/// product page's address.
async function importedShop(page: Page, aliexpress: FakeAliexpress): Promise<string> {
  await connectedShop(page, "Ali Sync Shop");
  expect(aliexpress.calls).toContain("/auth/token/create");
  await page.getByLabel("AliExpress product").fill(PRODUCT_ID);
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const row = page.locator(`[data-aliexpress-link="${PRODUCT_ID}"]`);
  await expect(async () => {
    await page.reload();
    await expect(row).toHaveAttribute("data-link-status", "linked", { timeout: 1000 });
  }).toPass({ timeout: 20_000 });
  await row.getByRole("link", { name: "Open" }).click();
  return page.url().split("?")[0];
}

test("the dashboard reads imported products again on its own, and renews the account's access", async ({
  page,
}) => {
  const aliexpress = new FakeAliexpress();
  // A token that lapses within the day is renewed by the next sweep.
  aliexpress.tokenLifetime = 60;
  await aliexpress.start();
  try {
    const productUrl = await importedShop(page, aliexpress);
    const rose = page.locator("[data-variant]", { hasText: "Colour: Rose" });
    await expect(rose.locator("[data-stock]")).toHaveAttribute("data-stock", "120");

    // Nobody presses anything: the sweep follows AliExpress.
    aliexpress.sku("12000037001").stock = 3;
    aliexpress.sku("12000037001").price = "5.00";
    // (A read counts the stock before it prices: wait for both on one page.)
    await expect(async () => {
      await page.goto(productUrl);
      await expect(rose.locator("[data-stock]")).toHaveAttribute("data-stock", "3", { timeout: 1000 });
      await expect(rose.getByLabel("Price of Colour: Rose")).toHaveValue("10.00", { timeout: 1000 });
    }).toPass({ timeout: 30_000 });
    await expect(async () => {
      expect(aliexpress.calls).toContain("/auth/token/refresh");
    }).toPass({ timeout: 15_000 });
    expect(aliexpress.badSignatures).toBe(0);
  } finally {
    await aliexpress.stop();
  }
});

test("a connected shop with nothing imported still has its access renewed", async ({ page }) => {
  const aliexpress = new FakeAliexpress();
  aliexpress.tokenLifetime = 60;
  await aliexpress.start();
  try {
    await connectedShop(page, "Ali Idle Shop");
    // Shops of the other specs are swept too: this one's renewal is the one
    // asked with the token its connection got.
    const issued = aliexpress.refreshTokens.at(-1);
    await expect(async () => {
      expect(aliexpress.renewedWith).toContain(issued);
    }).toPass({ timeout: 15_000 });
    expect(aliexpress.badSignatures).toBe(0);
  } finally {
    await aliexpress.stop();
  }
});

// Order forwarding: the shop's paid orders are bought on AliExpress for the
// shopper, delivered free by AliExpress itself, and fulfilled with the parcel
// number once it ships. The Chatounette example runs on a port of its own.
const app = new ExampleApp("chatounette", Number(process.env.E2E_ALIEXPRESS_SHOP_PORT ?? 3458));

test.afterEach(() => app.stop());

/// A shopper fills the Chatounette example's checkout for `quantity` roses,
/// stopping short of paying.
async function fillCheckout(page: Page, quantity: number) {
  await page.goto(`${app.url}/p/cat-tunnel-deluxe`);
  await page.getByLabel("Colour").selectOption("Rose");
  await page.getByLabel("Quantité").fill(String(quantity));
  await page.getByRole("button", { name: "Ajouter au panier" }).click();
  await expect(page.locator("[data-cart-drawer].open .drawer-panel")).toBeVisible();
  await page.goto(`${app.url}/cart`);
  await page.getByRole("link", { name: "Commander" }).click();
  await page.getByLabel("E-mail", { exact: true }).fill("ada@example.org");
  await page.getByLabel("Nom complet").fill("Ada Lovelace");
  await page.getByLabel("Adresse", { exact: true }).fill("1 rue de la Paix");
  await page.getByLabel("Code postal", { exact: true }).fill("75002");
  await page.getByLabel("Ville", { exact: true }).fill("Paris");
  await page.getByLabel("Pays", { exact: true }).selectOption("FR");
  await page.getByLabel("Téléphone").fill("06 12 34 56 78");
}

/// A shopper buys `quantity` roses on the Chatounette example and pays: the
/// order's id.
async function buyRoses(
  page: Page,
  request: APIRequestContext,
  stripe: FakeStripe,
  webhookUrl: string,
  quantity: number,
  address?: () => Promise<void>,
) {
  await fillCheckout(page, quantity);
  await address?.();
  await page.getByRole("button", { name: /^Payer ma commande/ }).click();
  await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
  const intent = stripe.lastIntent()!;
  stripe.pay(intent.id);
  const body = JSON.stringify({
    id: `evt_${intent.id}`,
    type: "payment_intent.succeeded",
    data: { object: { id: intent.id, object: "payment_intent" } },
  });
  const paid = await request.post(webhookUrl, {
    data: body,
    headers: { "content-type": "application/json", "stripe-signature": stripeSignature(body, "whsec_e2e") },
  });
  expect(paid.status()).toBe(204);
  // The return page looks again on its own; only sent there when the
  // browser left it (see shop.ts).
  if (!page.url().includes("/checkout/complete")) {
    await page.goto(`${app.url}/checkout/complete?redirect_status=succeeded`);
  }
  await expect(page).toHaveURL(new RegExp(`${app.url}/orders/[0-9a-f]{64}$`), { timeout: 15_000 });
  return page.url().split("/").at(-1)!;
}

/// Reloads the order's page until its AliExpress card says `status`.
async function forwardIs(page: Page, url: string, status: string) {
  await expect(async () => {
    await page.goto(url);
    await expect(page.locator("[data-order-aliexpress]")).toHaveAttribute("data-order-aliexpress", status, {
      timeout: 1000,
    });
  }).toPass({ timeout: 20_000 });
}

test("a shopper picks AliExpress's province and the order goes in its words", async ({ page, request }) => {
  test.setTimeout(150_000);
  await fakeStripeJs(page);
  const aliexpress = new FakeAliexpress();
  const stripe = new FakeStripe();
  aliexpress.places.BE = [
    {
      name: "Region Wallonne",
      type: "PROVINCE",
      children: [{ name: "Hainaut", type: "CITY" }],
    },
  ];
  aliexpress.places.IT = [
    {
      name: "Sicilia",
      type: "PROVINCE",
      children: [
        { name: "Catania", type: "CITY", children: [{ name: "Aci Castello", type: "TOWN" }] },
        { name: "Other", type: "CITY" },
      ],
    },
    {
      name: "Lazio",
      type: "PROVINCE",
      children: [{ name: "Roma", type: "CITY", children: [{ name: "Albano Laziale", type: "TOWN" }] }],
    },
  ];
  await aliexpress.start();
  await stripe.start();
  try {
    const productUrl = await importedShop(page, aliexpress);
    const slug = new URL(productUrl).pathname.split("/")[2];
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await app.start(request, await mintKey(page, slug, true), 1);

    const url = `/t/${slug}/orders/${await buyRoses(page, request, stripe, webhookUrl, 1, async () => {
      const province = page.getByLabel("Région / Province");
      // France has no list here: nothing to pick.
      await expect(province).toBeHidden();
      // Belgium's list holds provinces, so its town goes with the street.
      const hint = page.locator("[data-commune-hint]");
      await expect(hint).toBeHidden();
      await page.getByLabel("Pays", { exact: true }).selectOption("BE");
      await page.getByLabel("Code postal", { exact: true }).fill("6000");
      await page.getByLabel("Code postal", { exact: true }).blur();
      await expect(province).toHaveValue("Region Wallonne");
      await expect(page.getByLabel("Province / district").locator("option[value=Hainaut]")).toHaveCount(1);
      await expect(hint).toBeVisible();
      await page.getByLabel("Pays", { exact: true }).selectOption("IT");
      await expect(hint).toBeHidden();
      await page.getByLabel("Code postal", { exact: true }).fill("95021");
      await page.getByLabel("Code postal", { exact: true }).blur();
      // The postal code picks the province, and the city is picked from its
      // list. A town the list lacks is typed only under its "Other".
      await expect(province).toHaveValue("Sicilia");
      const city = page.getByLabel("Ville", { exact: true });
      const town = page.getByLabel("Commune", { exact: true });
      await expect(city.locator("option[value=Catania]")).toHaveCount(1);
      await city.selectOption("Other");
      await expect(page.getByLabel("Votre ville")).toBeVisible();
      await expect(town).toBeHidden();
      // A city with towns has them picked from, and nothing is typed.
      await city.selectOption("Catania");
      await expect(page.getByLabel("Votre ville")).toHaveCount(0);
      await expect(page.getByRole("textbox", { name: "Ville" })).toHaveCount(0);
      await expect(town.locator("option[value='Aci Castello']")).toHaveCount(1);
      // The shopper's pick stays when the postal code changes.
      await province.selectOption("Lazio");
      await expect(city.locator("option[value=Roma]")).toHaveCount(1);
      await page.getByLabel("Code postal", { exact: true }).fill("95022");
      await page.getByLabel("Code postal", { exact: true }).blur();
      await expect(province).toHaveValue("Lazio");
      await city.selectOption("Roma");
      await town.selectOption("Albano Laziale");
    })}`;
    await page.goto(url);
    await page.getByRole("button", { name: "Place on AliExpress" }).click();
    await forwardIs(page, url, "placed");
    // In AliExpress's words, the shopper's pick over the postal code's, with
    // their town kept for the courier.
    expect(aliexpress.orders[0].address).toMatchObject({
      province: "Lazio",
      city: "Roma",
      address2: "Albano Laziale",
      zip: "95022",
      country: "IT",
    });
  } finally {
    await stripe.stop();
    await aliexpress.stop();
  }
});

test("a shop places its paid orders on AliExpress and fulfils them with the tracking", async ({ page, request }) => {
  test.setTimeout(150_000);
  await fakeStripeJs(page);
  const aliexpress = new FakeAliexpress();
  const stripe = new FakeStripe();
  await aliexpress.start();
  await stripe.start();
  try {
    const productUrl = await importedShop(page, aliexpress);
    const slug = new URL(productUrl).pathname.split("/")[2];
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");
    await page.goto(`/t/${slug}/payments`);
    const webhookUrl = (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await app.start(request, await mintKey(page, slug, true), 1);

    // By hand first: only the seller delivers, untracked, so the order is not placed
    // and says why, with what was offered.
    await page.goto(`/t/${slug}/aliexpress`);
    await expect(page.locator("[data-aliexpress-forwarding]")).toHaveAttribute("data-aliexpress-forwarding", "manual");
    aliexpress.freight = [{ service_name: "YANWEN_JYT", cents: 120, tracking: false }];
    const first = `/t/${slug}/orders/${await buyRoses(page, request, stripe, webhookUrl, 1)}`;
    await page.goto(first);
    await expect(page.locator("[data-order-aliexpress]")).toHaveAttribute("data-order-aliexpress", "none");
    await page.getByRole("button", { name: "Place on AliExpress" }).click();
    await expect(page.getByRole("status")).toContainText("The order is being placed on AliExpress.");
    await forwardIs(page, first, "failed");
    await expect(page.locator("[data-order-aliexpress-error]")).toContainText(
      "No delivery of “Cat Tunnel Deluxe” to FR on AliExpress can be followed; offered: YANWEN_JYT (1.20 EUR).",
    );
    expect(aliexpress.orders).toHaveLength(0);

    // Shipped by AliExpress again: placed, for the merchant to pay in
    // AliExpress.
    aliexpress.freight = [
      { service_name: "CAINIAO_PREMIUM", cents: 990, tracking: true },
      { service_name: "CAINIAO_STANDARD", cents: 0, tracking: true },
    ];
    await page.getByRole("button", { name: "Place again" }).click();
    await forwardIs(page, first, "placed");
    expect(aliexpress.orders).toHaveLength(1);
    const manual = aliexpress.orders[0];
    expect(manual.tryToPay).toBe(false);
    expect(manual.items).toEqual([
      {
        product_id: Number(PRODUCT_ID),
        sku_attr: aliexpress.skuAttr(aliexpress.sku("12000037001")),
        product_count: 1,
        logistics_service_name: "CAINIAO_STANDARD",
        order_memo: expect.any(String),
      },
    ]);
    expect(manual.address).toMatchObject({
      full_name: "Ada Lovelace",
      address: "1 rue de la Paix",
      zip: "75002",
      city: "Paris",
      province: "Paris",
      country: "FR",
      phone_country: "+33",
      mobile_no: "612345678",
    });
    await expect(page.locator(`[data-order-aliexpress-order="${manual.id}"]`)).toBeVisible();
    await expect(page.locator("[data-order-aliexpress]")).toContainText("Pay it in AliExpress.");
    await expect(page.getByRole("button", { name: "Place again" })).toHaveCount(0);

    // Then as they come, paid by AliExpress: the order is placed on its
    // own, and once its parcel ships the order is fulfilled with it.
    await page.goto(`/t/${slug}/aliexpress`);
    await page.getByLabel("Place paid orders on AliExpress automatically").check();
    await page.getByLabel("Ask AliExpress to pay them with the account's saved payment method").check();
    await page.getByLabel("Phone for orders whose shopper gave none").fill("01 89 76 02 12");
    await page.getByRole("button", { name: "Save orders settings" }).click();
    await expect(page.getByRole("status")).toContainText("Orders settings saved.");
    await expect(page.getByLabel("Phone for orders whose shopper gave none")).toHaveValue("01 89 76 02 12");
    await expect(page.locator("[data-aliexpress-forwarding]")).toHaveAttribute("data-aliexpress-forwarding", "auto");
    const second = `/t/${slug}/orders/${await buyRoses(page, request, stripe, webhookUrl, 2)}`;
    await forwardIs(page, second, "placed");
    const auto = aliexpress.orders[1];
    expect(auto.tryToPay).toBe(true);
    expect(auto.items[0].product_count).toBe(2);
    await expect(page.locator("[data-order-status]")).toHaveAttribute("data-order-status", "paid");

    // AliExpress pushes the order's new status to the platform's webhook
    // (its message push, signed with the app's key and secret): the order
    // is read again at once, without waiting for the sweep. A push that
    // does not check out, or is not a push, is refused; another topic, or
    // an order no shop placed, is taken and let go.
    const pushUrl = "/aliexpress/webhook";
    aliexpress.ship(auto.id, "LP00123456FR");
    const readsBefore = aliexpress.orderReads();
    const pushed = await aliexpress.push(request, pushUrl, auto.id, "OrderShipped");
    expect(pushed.status()).toBe(200);
    await expect.poll(() => aliexpress.orderReads(), { timeout: 5_000 }).toBeGreaterThan(readsBefore);
    await forwardIs(page, second, "done");
    const forged = await aliexpress.push(request, pushUrl, auto.id, "OrderShipped", { signature: "0".repeat(64) });
    expect(forged.status()).toBe(401);
    const unknown = await aliexpress.push(request, pushUrl, "8199999999999999", "OrderShipped");
    expect(unknown.status()).toBe(200);
    const otherTopic = await aliexpress.push(request, pushUrl, auto.id, "", {
      body: JSON.stringify({ seller_id: "1234", message_type: 1, data: { trade_order_id: auto.id }, timestamp: 1, site: "ae_global" }),
    });
    expect(otherTopic.status()).toBe(200);
    // (Valid JSON: Playwright re-encodes any other string before sending.)
    const malformed = await aliexpress.push(request, pushUrl, auto.id, "", { body: JSON.stringify({ hello: "world" }) });
    expect(malformed.status()).toBe(400);
    await expect(page.locator("[data-order-status]")).toHaveAttribute("data-order-status", "fulfilled");
    await expect(page.locator("[data-order-tracking]")).toHaveText("LP00123456FR");
    await expect(page.locator("[data-order-aliexpress-tracking]")).toHaveText("LP00123456FR");
    expect(aliexpress.badSignatures).toBe(0);
  } finally {
    await stripe.stop();
    await aliexpress.stop();
  }
});

test("a shopper pays what AliExpress asks at the moment of paying", async ({ page, request }) => {
  test.setTimeout(150_000);
  await fakeStripeJs(page);
  const aliexpress = new FakeAliexpress();
  const stripe = new FakeStripe();
  await aliexpress.start();
  await stripe.start();
  try {
    const productUrl = await importedShop(page, aliexpress);
    const slug = new URL(productUrl).pathname.split("/")[2];
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");
    await page.goto(`/t/${slug}/payments`);
    await page.getByLabel("Publishable key").fill("pk_test_e2e");
    await page.getByLabel("Secret key").fill("sk_test_e2e");
    await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Stripe connected.");
    await app.start(request, await mintKey(page, slug, true), 1);

    await fillCheckout(page, 1);
    const pay = page.getByRole("button", { name: /^Payer ma commande/ });
    const total = page.locator("[data-place-total]");
    await expect(total).toContainText("8,62");

    // AliExpress ran out while the shopper filled the form: nothing is opened
    // with Stripe, so nothing has to be refunded later.
    aliexpress.sku("12000037001").stock = 0;
    await pay.click();
    await expect(page.getByText("il n'en reste pas assez")).toBeVisible();
    expect(stripe.lastIntent()).toBeUndefined();

    // It has some again, dearer; the checkout says so once read.
    aliexpress.sku("12000037001").stock = 120;
    aliexpress.sku("12000037001").price = "5.00";
    await expect(async () => {
      await page.goto(`${app.url}/checkout`);
      await expect(total).toContainText("10,00", { timeout: 1000 });
    }).toPass({ timeout: 20_000 });

    // The price moves again before the shopper pays: refused, with the new
    // total to confirm, and the payment opens at that total.
    aliexpress.sku("12000037001").price = "6.00";
    await pay.click();
    await expect(page.getByText("Un prix a changé")).toBeVisible();
    await expect(total).toContainText("12,00");
    expect(stripe.lastIntent()).toBeUndefined();
    await pay.click();
    await expect(page).toHaveURL(/\/checkout\/complete\?.*redirect_status=succeeded/, { timeout: 30_000 });
    expect(stripe.lastIntent()!.amount).toBe(1200);
    expect(aliexpress.badSignatures).toBe(0);
  } finally {
    await stripe.stop();
    await aliexpress.stop();
  }
});

test("a small move in what AliExpress asks leaves a price alone, by the shop's tolerance", async ({ page }) => {
  const aliexpress = new FakeAliexpress();
  await aliexpress.start();
  try {
    const productUrl = await importedShop(page, aliexpress);
    const slug = new URL(productUrl).pathname.split("/")[2];
    const reads = () => aliexpress.calls.filter((call) => call === "aliexpress.ds.product.get").length;

    await page.goto(`/t/${slug}/aliexpress`);
    await page.getByLabel("Price tolerance (%)").fill("21");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page).toHaveURL(/error=invalid_price_tolerance/);
    await page.getByLabel("Price tolerance (%)").fill("5");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status")).toContainText("AliExpress settings saved.");
    await expect(page.getByLabel("Price tolerance (%)")).toHaveValue("5.00");

    // 4.40 gives 8.80, within 5 % of 8.62: the sweep reads it and leaves it.
    await page.goto(productUrl);
    const price = page.getByLabel("Price of Colour: Rose");
    await expect(price).toHaveValue("8.62");
    aliexpress.sku("12000037001").price = "4.40";
    const before = reads();
    await expect.poll(reads, { timeout: 20_000 }).toBeGreaterThan(before + 1);
    await page.reload();
    await expect(price).toHaveValue("8.62");

    // A member asking gets the rule's price to the cent.
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.getByRole("status")).toContainText("Read again from AliExpress.");
    await expect(price).toHaveValue("8.80");

    // Past the tolerance, the sweep follows on its own.
    aliexpress.sku("12000037001").price = "5.00";
    await expect(async () => {
      await page.goto(productUrl);
      await expect(price).toHaveValue("10.00", { timeout: 1000 });
    }).toPass({ timeout: 20_000 });
  } finally {
    await aliexpress.stop();
  }
});
