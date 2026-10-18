import { expect, test } from "@playwright/test";
import { APP_KEY, APP_SECRET, FakeAliexpress, PRODUCT_ID } from "./aliexpress";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// AliExpress end to end against a stand-in (aliexpress.ts): a merchant sets up
// the shop's own app, connects the AliExpress account it acts for, imports a
// product by its address (a draft with its options, pictures, stock and
// prices by the shop's rule), then reads it again: stock and price follow
// AliExpress, and a product AliExpress stops selling is taken off sale.

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
    await expect(page.locator("[data-aliexpress-callback]")).toHaveText(
      new RegExp(`/t/${slug}/aliexpress/callback$`),
    );

    // The app: a rule that is not one is refused; then saved.
    await page.getByLabel("App key").fill(APP_KEY);
    await page.getByLabel("App secret").fill(APP_SECRET);
    await page.getByLabel("Ships to (country)").fill("FR");
    await page.getByLabel("Worded in (language)").fill("fr");
    await page.getByLabel("Multiply the cost by").fill("0.5");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page).toHaveURL(/error=invalid_pricing_rule/);
    await page.getByLabel("App key").fill(APP_KEY);
    await page.getByLabel("App secret").fill(APP_SECRET);
    await page.getByLabel("Multiply the cost by").fill("2");
    await page.getByLabel("Round").selectOption("99");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("AliExpress app saved.");
    await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "configured");
    await expect(page.locator("[data-aliexpress-rule]")).toHaveText("Cost ×2.00, up to .99");

    // The account says yes on AliExpress, and the shop is connected.
    await page.getByRole("button", { name: "Connect the AliExpress account" }).click();
    await expect(page.getByRole("status")).toContainText("AliExpress account connected.");
    await expect(page.locator("[data-aliexpress]")).toHaveAttribute("data-aliexpress", "connected");
    await expect(page.locator("[data-aliexpress-account]")).toHaveText("shop@example.org");

    // An address that is not an item's is refused; an item's is imported.
    await page.getByLabel("AliExpress product").fill("https://example.com/p/12");
    await page.getByRole("button", { name: "Import" }).click();
    await expect(page).toHaveURL(/error=invalid_aliexpress_product/);
    await page.getByLabel("AliExpress product").fill(`https://fr.aliexpress.com/item/${PRODUCT_ID}.html?spm=e2e`);
    await page.getByRole("button", { name: "Import" }).click();
    await expect(page.getByRole("status")).toContainText("Import started");
    const row = page.locator(`[data-aliexpress-link="${PRODUCT_ID}"]`);
    await expect(async () => {
      await page.reload();
      await expect(row).toHaveAttribute("data-link-status", "linked", { timeout: 1000 });
    }).toPass({ timeout: 20_000 });

    // A draft with both colours, their pictures and stock, priced by the rule.
    await row.getByRole("link", { name: "Open" }).click();
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Cat Tunnel Deluxe");
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "draft");
    await expect(page.locator("[data-image]")).toHaveCount(4);
    const rose = page.locator("[data-variant]", { hasText: "Colour: Rose" });
    const blue = page.locator("[data-variant]", { hasText: "Colour: Blue" });
    await expect(rose.locator("[data-stock]")).toHaveAttribute("data-stock", "120");
    await expect(blue.locator("[data-stock]")).toHaveAttribute("data-stock", "0");
    await expect(rose.getByLabel("Price of Colour: Rose")).toHaveValue("8.99");
    await expect(blue.getByLabel("Price of Colour: Blue")).toHaveValue("11.99");
    await expect(page.locator("[data-product-aliexpress]")).toHaveAttribute("data-product-aliexpress", PRODUCT_ID);
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");

    // AliExpress has fewer roses, dearer: the next read follows.
    aliexpress.sku("12000037001").stock = 7;
    aliexpress.sku("12000037001").price = "6.00";
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.getByRole("status")).toContainText("Read again from AliExpress.");
    await expect(rose.locator("[data-stock]")).toHaveAttribute("data-stock", "7");
    await expect(rose.getByLabel("Price of Colour: Rose")).toHaveValue("12.99");

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
    await page.getByRole("button", { name: "Import" }).click();
    await expect(page).toHaveURL(/error=aliexpress_already_linked/);
    expect(aliexpress.badSignatures).toBe(0);
    expect(aliexpress.calls).toContain("/auth/token/create");
    expect(aliexpress.calls.filter((call) => call === "aliexpress.ds.product.get").length).toBeGreaterThanOrEqual(4);
  } finally {
    await aliexpress.stop();
  }
});
