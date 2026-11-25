import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// The tuning of a shop's first campaign, as far as a browser reaches it
// without three weeks of numbers: the page waits for a campaign, the
// thresholds a round decides on are the shop's to set, and a product is
// kept out of ads by hand as a round would propose it. What a round reads,
// proposes and changes at Google Ads is covered by the dashboard's own
// tests against a stand-in (dashboard/src/tuning/tests.rs).

test("a shop sets its tuning thresholds and keeps a product out of ads", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("tuning"));
  const shop = uniqueTenant("Tuning");
  await createTenant(page, shop.name);

  // Reached from the campaigns; nothing to tune before a campaign.
  await page.goto(`/t/${shop.slug}/ads/campaigns`);
  await page.getByRole("link", { name: "Tuning", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/ads/tuning$`));
  await expect(page.locator("[data-tuning]")).toHaveAttribute("data-tuning", "waits");
  await expect(page.locator("[data-tuning-waits]")).toContainText("no first campaign yet");
  await expect(page.getByRole("button", { name: "Prepare a round" })).toHaveCount(0);

  // The defaults, then the shop's own.
  const thresholds = page.locator("[data-tuning-thresholds]");
  await expect(thresholds.getByLabel("Days read")).toHaveValue("21");
  await expect(thresholds.getByLabel("Clicks before a bid moves")).toHaveValue("100");
  await expect(thresholds.getByLabel("Bid step")).toHaveValue("0.02");
  await expect(thresholds.getByLabel("A round every so many days")).toHaveValue("0");
  await thresholds.getByLabel("Clicks before a bid moves").fill("60");
  await thresholds.getByLabel("Bid step").fill("0,03");
  await thresholds.getByLabel("A round every so many days").fill("14");
  await thresholds.getByRole("button", { name: "Save the thresholds" }).click();
  await expect(page.getByRole("status")).toContainText("Thresholds saved.");
  await expect(thresholds.getByLabel("Clicks before a bid moves")).toHaveValue("60");
  await expect(thresholds.getByLabel("Bid step")).toHaveValue("0.03");
  await expect(thresholds.getByLabel("A round every so many days")).toHaveValue("14");
  // By itself a round comes a week apart at least.
  await thresholds.getByLabel("A round every so many days").fill("3");
  await thresholds.getByRole("button", { name: "Save the thresholds" }).click();
  await expect(page.getByRole("alert")).toContainText("A threshold is a whole number within its range");
  await expect(thresholds.getByLabel("A round every so many days")).toHaveValue("14");

  // A product kept out of ads stays in the shop.
  await page.goto(`/t/${shop.slug}/products`);
  await page.getByLabel("Name").fill("Arbre a chat");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const productUrl = page.url().split("?")[0];
  await page.goto(`${productUrl}?tab=shopping`);
  const ads = page.locator("[data-product-ads]");
  await expect(ads).toHaveAttribute("data-product-ads", "in");
  await ads.getByLabel("Keep it out of ads").check();
  await ads.getByRole("button", { name: "Save ads" }).click();
  await expect(page.getByRole("status")).toContainText("Kept out of ads.");
  await expect(ads).toHaveAttribute("data-product-ads", "out");
  await ads.getByLabel("Keep it out of ads").uncheck();
  await ads.getByRole("button", { name: "Save ads" }).click();
  await expect(page.getByRole("status")).toContainText("It may be advertised.");
  await expect(ads).toHaveAttribute("data-product-ads", "in");
});
