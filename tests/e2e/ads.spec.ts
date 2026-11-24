import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { FakeGoogle, searchDay } from "./google";
import { createTenant, uniqueTenant } from "./tenants";

// What Google Ads says a shop's account cost and brought, end to end
// against a stand-in for Google (google.ts): a shop that connected its
// Google account chooses which of the Google Ads accounts it reaches is its
// own, and the account is read off the page: the period's totals, then the
// same by campaign, device, ad group, category, product and search, each
// product told for what it is in the catalog. A read Google does not answer
// says why and when it is tried again.

test("a shop chooses its Google Ads account and reads what its ads cost and brought", async ({ page }) => {
  const google = new FakeGoogle();
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("ads"));
    const shop = uniqueTenant("Ads");
    await createTenant(page, shop.name);

    // A product of the shop, which one of the feed's items shown is. Google
    // Ads reports an item's id in lower case.
    await page.goto(`/t/${shop.slug}/products`);
    await page.getByLabel("Name").fill("Arbre a chat");
    await page.getByRole("button", { name: "Create product" }).click();
    await expect(page.getByRole("status")).toContainText("Product created.");
    const productId = new URL(page.url()).pathname.split("/").at(-1)!;
    google.adsItems = [productId.toLowerCase(), "gone-item"];
    // Yesterday and the day before, on the account's clock.
    google.adsDays = [searchDay(2), searchDay(1)];
    google.adsAccounts = [
      { id: "1111111111", name: "Acme", currency: "EUR", timeZone: "UTC" },
      { id: "4444444444", name: "Client", currency: "USD", timeZone: "UTC", manager: "2222222222" },
    ];

    // Nothing to read through yet.
    await page.getByRole("link", { name: "Google Ads", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/ads$`));
    await expect(page.locator("[data-ads]")).toHaveAttribute("data-ads", "not-connected");
    await page.locator("[data-ads]").getByRole("link", { name: "Google account" }).click();

    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("status")).toContainText("Google account connected.");
    await page.goto(`/t/${shop.slug}/ads`);
    await expect(page.locator("[data-ads]")).toHaveAttribute("data-ads", "no-account");

    // The accounts the grant reaches: its own, and a manager's client.
    await page.goto(`/t/${shop.slug}/google`);
    await expect(page.locator("[data-google-ads-missing]")).toBeVisible();
    const accounts = page.getByRole("group", { name: "The Google Ads accounts this Google account reaches" });
    await expect(accounts.getByRole("radio")).toHaveCount(2);
    await expect(accounts.getByText("through a manager account")).toHaveCount(1);
    await accounts.getByRole("radio", { name: /111-111-1111/ }).check();
    await page.getByRole("button", { name: "Save the Google Ads account" }).click();
    await expect(page.getByRole("status")).toContainText("Google Ads account saved.");
    await expect(page.locator("[data-google-ads]")).toHaveAttribute("data-google-ads", "1111111111");
    await expect(page.locator("[data-google-ads]")).toContainText("111-111-1111 · Acme");
    // The list is asked of Google only to choose.
    await expect(accounts).toHaveCount(0);

    // Read off the page: the two days the ads were shown on.
    await page.goto(`/t/${shop.slug}/ads`);
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-ads-days]")).toHaveAttribute("data-ads-days", "2", { timeout: 1000 });
    }).toPass({ timeout: 30_000 });
    await expect(page.locator("[data-ads]")).toHaveAttribute("data-ads", "read");
    await expect(page.locator("[data-ads-totals]")).toHaveAttribute("data-ads-totals", "EUR");
    // 12.00 a day for 40 clicks and 40.00 of conversions.
    await expect(page.locator("[data-ads-cost]")).toHaveAttribute("data-ads-cost", "2400");
    await expect(page.locator("[data-ads-clicks]")).toHaveAttribute("data-ads-clicks", "80");
    await expect(page.locator("[data-ads-value]")).toHaveAttribute("data-ads-value", "8000");
    await expect(page.locator("[data-ads-return]")).toHaveText("3.33");
    await expect(page.locator("[data-ads-period]")).toContainText(`to ${searchDay(1)}`);
    await expect(page.locator("[data-ads-period]")).toContainText("Acme (111-111-1111)");

    // Four reports a pass, each one question over a range of days, all of
    // the account chosen, up to its yesterday.
    expect(google.adsQuestions.every((asked) => asked.customer === "1111111111")).toBe(true);
    expect(google.adsQuestions.slice(0, 4).map((asked) => asked.report)).toEqual([
      "campaign",
      "ad_group",
      "shopping_performance_view",
      "search_term_view",
    ]);
    expect(google.adsQuestions.slice(0, 4).every((asked) => asked.last === searchDay(1))).toBe(true);

    const table = (name: string) => page.locator(`[data-ads-table="${name}"]`);
    await expect(table("campaigns").locator("[data-ads-row]")).toHaveCount(1);
    await expect(table("campaigns").locator('[data-ads-row="11"]')).toContainText("Shopping");
    await expect(table("campaigns").locator('[data-ads-row="11"] [data-cost]')).toHaveAttribute("data-cost", "2400");
    await expect(table("devices").locator("[data-ads-row]").nth(0)).toContainText("Phones");
    await expect(table("devices").locator('[data-ads-row="desktop"] [data-clicks]')).toHaveAttribute("data-clicks", "20");
    await expect(table("ad-groups").locator('[data-ads-row="21"]')).toContainText("Arbres à chat");
    await expect(table("ad-groups").locator('[data-ads-row="22"] [data-return]')).toHaveText("0.00");

    // The products, each told for what it is in the catalog now.
    const product = table("products").locator(`[data-ads-row="${productId.toLowerCase()}"]`);
    await expect(product.getByRole("link", { name: "Arbre a chat" })).toHaveAttribute(
      "href",
      new RegExp(`/t/${shop.slug}/products/${productId}$`),
    );
    await expect(product.locator("[data-return]")).toHaveText("5.00");
    await expect(table("products").locator('[data-ads-row="gone-item"]')).toContainText("gone-item");
    await expect(table("categories").locator('[data-ads-row="none"]')).toContainText("In no category");
    await expect(table("categories").locator('[data-ads-row="gone"]')).toContainText("No longer in the catalog");
    await expect(table("terms").locator("[data-ads-row]").nth(0)).toHaveAttribute("data-ads-row", "arbre à chat");

    // A shorter period holds the same two days.
    await page.getByLabel("Period").selectOption("7");
    await page.getByRole("button", { name: "Show" }).click();
    await expect(page).toHaveURL(/days=7/);
    await expect(page.locator("[data-ads-days]")).toHaveAttribute("data-ads-days", "2");

    // The sales report sets the spend against what the shop sold: nothing
    // yet, so the ads only cost.
    await page.getByRole("link", { name: "Sales report", exact: true }).click();
    await expect(page.locator("[data-sales-ads]")).toHaveAttribute("data-sales-ads", "connected");
    const channel = page.locator('[data-sales-channel="google-ads"]');
    await expect(channel).toContainText("Google Ads");
    await expect(channel.locator("[data-orders]")).toHaveAttribute("data-orders", "0");
    await expect(channel.locator("[data-spend]")).toHaveAttribute("data-spend", "2400");
    await expect(channel.locator("[data-after-ads]")).toHaveAttribute("data-after-ads", "-2400");
    const spent = page.locator(`[data-sales-ad-table="products"] [data-sales-ad-row="${productId}"]`);
    await expect(spent.getByRole("link", { name: "Arbre a chat" })).toBeVisible();
    await expect(spent.locator("[data-spend]")).toHaveAttribute("data-spend", "1600");
    await expect(spent.locator("[data-return]")).toHaveText("0.00");
    await expect(spent.locator("[data-verdict]")).toHaveAttribute("data-verdict", "Costs");
    await expect(page.locator('[data-sales-ad-table="categories"] [data-sales-ad-row="none"]')).toContainText(
      "In no category",
    );

    // The same in the file.
    const downloading = page.waitForEvent("download");
    await page.getByRole("link", { name: "Export these days as CSV" }).click();
    const download = await downloading;
    expect(download.suggestedFilename()).toMatch(new RegExp(`^sales-${shop.slug}-\\d{4}-\\d\\d-\\d\\d-`));
    const href = await page.getByRole("link", { name: "Export these days as CSV" }).getAttribute("href");
    const file = await (await page.request.get(href!)).text();
    expect(file).toContain("section,key,name,currency,orders,quantity,revenue,refunds,profit,ad_spend,");
    expect(file).toContain("channel,google-ads,Google Ads,EUR,0,,0.00,0.00,,24.00,0.00,,-24.00");
    expect(file).toContain(`product,${productId},Arbre a chat,EUR,,0,0.00,,,16.00,0.00,,-16.00`);
  } finally {
    await google.stop();
  }
});

test("a read Google Ads does not answer says why and when it is tried again", async ({ page }) => {
  const google = new FakeGoogle();
  google.adsFails = true;
  google.adsAccounts = [{ id: "1111111111", name: "Acme", currency: "EUR", timeZone: "UTC" }];
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("ads"));
    const shop = uniqueTenant("Ads");
    await createTenant(page, shop.name);
    await page.goto(`/t/${shop.slug}/google`);
    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("status")).toContainText("Google account connected.");
    await page.getByRole("radio", { name: /111-111-1111/ }).check();
    await page.getByRole("button", { name: "Save the Google Ads account" }).click();
    await expect(page.getByRole("status")).toContainText("Google Ads account saved.");

    await page.goto(`/t/${shop.slug}/ads`);
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-ads-failure]")).toContainText(
        "The last read of Google Ads failed: Google is unavailable: Internal error encountered.",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 30_000 });
    await expect(page.locator("[data-ads-failure]")).toContainText("It is tried again from");
    await expect(page.locator("[data-ads-totals]")).toHaveCount(0);
  } finally {
    await google.stop();
  }
});
