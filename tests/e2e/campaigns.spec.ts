import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { FakeGoogle } from "./google";
import { fileUnder } from "./storefront";
import { createTenant, uniqueTenant } from "./tenants";

// A shop's first Shopping campaign, end to end against a stand-in for
// Google (google.ts): the page says what the shop still waits for, a
// member fills the campaign's settings and prepares it, reads the whole of
// it as a proposal, changes a value and approves. Only then is it created
// at Google Ads, paused, with an ad group a category. Set running from the
// page, it says since when it runs and to leave it alone for three weeks.

test("a shop prepares its first campaign, approves it and sets it running", async ({ page }) => {
  const google = new FakeGoogle();
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("campaign"));
    const shop = uniqueTenant("Campaign");
    await createTenant(page, shop.name);

    // A category with a product in it, and one kept out of ads.
    await page.goto(`/t/${shop.slug}/categories`);
    await page.getByLabel("Name").fill("Arbres a chat");
    await page.getByRole("button", { name: "Create category" }).click();
    await expect(page.getByRole("status")).toContainText("Category created.");
    const categoryId = new URL(page.url()).pathname.split("/").at(-1)!;
    await page.goto(`/t/${shop.slug}/products`);
    await page.getByLabel("Name").fill("Arbre a chat");
    await page.getByRole("button", { name: "Create product" }).click();
    await expect(page.getByRole("status")).toContainText("Product created.");
    const productUrl = page.url().split("?")[0];
    const productId = new URL(productUrl).pathname.split("/").at(-1)!;
    await fileUnder(page, productUrl, "Arbres a chat");

    google.adsAccounts = [{ id: "1111111111", name: "Acme", currency: "EUR", timeZone: "UTC" }];
    google.merchantAccounts = [{ accountId: "5550001", accountName: "Acme" }];
    google.merchantProducts = [{ offerId: productId, ads: "approved", listings: "approved" }];

    // Nothing is ready: the page says what is missing, and offers no form.
    await page.goto(`/t/${shop.slug}/ads`);
    await page.getByRole("link", { name: "Campaigns", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/ads/campaigns$`));
    const shopping = page.locator("[data-campaign=shopping]");
    const check = (key: string) => shopping.locator(`[data-campaign-check=${key}]`);
    for (const key of ["account", "conversion", "products", "label"]) {
      await expect(check(key)).toHaveAttribute("data-met", "false");
    }
    await expect(shopping.locator("[data-campaign-not-ready]")).toBeVisible();
    await expect(shopping.locator("[data-campaign-form]")).toHaveCount(0);

    // The Google account, with its Google Ads and Merchant Center accounts.
    await page.goto(`/t/${shop.slug}/google`);
    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("status")).toContainText("Google account connected.");
    await page
      .getByRole("group", { name: "The Google Ads accounts this Google account reaches" })
      .getByRole("radio", { name: /111-111-1111/ })
      .check();
    await page.getByRole("button", { name: "Save the Google Ads account" }).click();
    await expect(page.getByRole("status")).toContainText("Google Ads account saved.");
    await page
      .getByRole("group", { name: "The Merchant Center accounts this Google account reaches" })
      .getByRole("radio", { name: /5550001/ })
      .check();
    await page.getByRole("button", { name: "Save the Merchant Center account" }).click();
    await expect(page.getByRole("status")).toContainText("Merchant Center account saved.");

    // The tag that counts the sales, and the feed's label for campaigns.
    await page.goto(`/t/${shop.slug}/google-tag`);
    const tag = page.locator("[data-google-tag]");
    await tag.getByLabel("Google Ads conversion ID").fill("AW-123456789");
    await tag.getByLabel("Label of the sale conversion").fill("SaLe_1");
    await tag.getByRole("button", { name: "Save the tag" }).click();
    await expect(page.getByRole("status")).toContainText("Tag saved");
    await page.goto(`/t/${shop.slug}/feed`);
    await page.getByLabel("Label for campaigns").selectOption({ label: "Custom label 3" });
    await page.getByRole("button", { name: "Save the label" }).click();
    await expect(page.getByRole("status")).toContainText("Label saved.");

    // Ready once Merchant Center was read: the form, with its defaults.
    await expect(async () => {
      await page.goto(`/t/${shop.slug}/ads/campaigns`);
      await expect(check("products")).toHaveAttribute("data-met", "true", { timeout: 1000 });
      await expect(check("categories")).toHaveAttribute("data-met", "true", { timeout: 1000 });
    }).toPass({ timeout: 30_000 });
    for (const key of ["account", "conversion", "label"]) {
      await expect(check(key)).toHaveAttribute("data-met", "true");
    }
    const form = shopping.locator("[data-campaign-form]");
    await expect(form.getByLabel("Campaign name")).toHaveValue(`Shopping · ${shop.name}`);
    await expect(form.getByLabel("Starting bid")).toHaveValue("0.30");
    await expect(form.getByLabel("Phones, in %")).toHaveValue("-55");
    await expect(form.getByLabel("Tablets, in %")).toHaveValue("-40");
    await expect(form.getByLabel("Starts")).toHaveValue("paused");
    await expect(form.getByLabel("Ad groups")).toHaveValue(`${categoryId} Arbres a chat`);

    // A bid above the day's budget is told at once.
    await form.getByLabel("Countries").fill("FR, BE");
    await form.getByLabel("Daily budget").fill("0.20");
    await form.getByRole("button", { name: "Prepare the campaign" }).click();
    await expect(page.getByRole("alert")).toContainText("The bid is more than the campaign may spend in a day.");

    // Prepared: a proposal, and nothing sent to Google Ads.
    await form.getByLabel("Countries").fill("FR, BE");
    await form.getByLabel("Daily budget").fill("15");
    await form.getByRole("button", { name: "Prepare the campaign" }).click();
    await expect(page.getByRole("status")).toContainText("The campaign is prepared.");
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/proposals/[^/?]+`));
    const proposalUrl = page.url().split("?")[0];
    await expect(page.locator("[data-proposal-status]")).toHaveAttribute("data-proposal-status", "waiting");
    expect(google.adsMutations).toHaveLength(0);
    const answer = page.locator("[data-proposal-answer]");
    await expect(answer.getByLabel("Daily budget")).toHaveValue("15.00");
    await expect(answer.getByLabel("Countries")).toHaveValue("FR, BE");
    await expect(answer.getByLabel("Ad groups")).toHaveValue(`${categoryId} Arbres a chat`);

    // While it waits, the campaign's page leads to it in place of a form.
    await expect(async () => {
      await page.goto(`/t/${shop.slug}/ads/campaigns`);
      await expect(shopping.locator("[data-campaign-waiting]")).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(shopping.locator("[data-campaign-form]")).toHaveCount(0);
    await shopping.getByRole("link", { name: "read the proposal" }).click();
    await expect(page).toHaveURL(proposalUrl);

    // Approved with the bid raised: created at Google Ads, paused.
    await answer.getByLabel("Starting bid").fill("0,35");
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("status")).toContainText("Approved");
    await expect(async () => {
      await page.goto(proposalUrl);
      await expect(page.locator("[data-proposal-status]")).toHaveAttribute("data-proposal-status", "applied", {
        timeout: 1000,
      });
    }).toPass({ timeout: 30_000 });
    expect(google.adsMutations).toHaveLength(1);
    expect(google.adsCampaigns).toHaveLength(1);
    const created = google.adsCampaigns[0];
    expect(created.name).toBe(`Shopping · ${shop.name}`);
    expect(created.status).toBe("PAUSED");
    expect(created.adGroups.map((group) => group.name)).toEqual(["Arbres a chat"]);
    const operations = google.adsMutations[0];
    const campaign = operations.find((o) => o.campaignOperation)!.campaignOperation.create;
    expect(campaign.shoppingSetting.merchantId).toBe("5550001");
    expect(operations.find((o) => o.campaignBudgetOperation)!.campaignBudgetOperation.create.amountMicros).toBe(
      "15000000",
    );
    expect(operations.find((o) => o.adGroupOperation)!.adGroupOperation.create.cpcBidMicros).toBe("350000");
    const shown = operations
      .filter((o) => o.adGroupCriterionOperation)
      .map((o) => o.adGroupCriterionOperation.create)
      .find((criterion) => criterion.listingGroup.caseValue?.productCustomAttribute.value);
    expect(shown.listingGroup.caseValue.productCustomAttribute).toEqual({
      index: "INDEX3",
      value: categoryId.toLowerCase(),
    });

    // The page says what Google Ads holds; set running, it says since when.
    await page.goto(`/t/${shop.slug}/ads/campaigns`);
    await expect(shopping.locator("[data-campaign-name]")).toContainText(`Shopping · ${shop.name} · number ${created.id}`);
    await expect(shopping.locator("[data-campaign-status]")).toHaveAttribute("data-campaign-status", "paused");
    await expect(shopping.locator("[data-campaign-settling]")).toHaveCount(0);
    await shopping.getByRole("button", { name: "Set it running" }).click();
    await expect(page.getByRole("status")).toContainText("Google Ads is being told to run the campaign.");
    await expect(async () => {
      await page.goto(`/t/${shop.slug}/ads/campaigns`);
      await expect(shopping.locator("[data-campaign-status]")).toHaveAttribute("data-campaign-status", "live", {
        timeout: 1000,
      });
    }).toPass({ timeout: 30_000 });
    expect(created.status).toBe("ENABLED");
    await expect(shopping.locator("[data-campaign-settling]")).toContainText("Leave it as it is for its first 21 days");
    await expect(shopping.getByRole("button", { name: "Set it running" })).toHaveCount(0);

    // The campaign on the shop's name waits for the shop's address.
    const brand = page.locator("[data-campaign=brand]");
    await expect(brand.locator("[data-campaign-check=address]")).toHaveAttribute("data-met", "false");
  } finally {
    await google.stop();
  }
});

test("a campaign Google Ads refuses says why on its proposal", async ({ page }) => {
  const google = new FakeGoogle();
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("refused"));
    const shop = uniqueTenant("Refused");
    await createTenant(page, shop.name);
    google.adsAccounts = [{ id: "1111111111", name: "Acme", currency: "EUR", timeZone: "UTC" }];
    google.adsRefusesCampaigns = true;

    await page.goto(`/t/${shop.slug}/google`);
    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("status")).toContainText("Google account connected.");
    await page
      .getByRole("group", { name: "The Google Ads accounts this Google account reaches" })
      .getByRole("radio", { name: /111-111-1111/ })
      .check();
    await page.getByRole("button", { name: "Save the Google Ads account" }).click();
    await expect(page.getByRole("status")).toContainText("Google Ads account saved.");
    await page.goto(`/t/${shop.slug}/google-tag`);
    const tag = page.locator("[data-google-tag]");
    await tag.getByLabel("Google Ads conversion ID").fill("AW-123456789");
    await tag.getByLabel("Label of the sale conversion").fill("SaLe_1");
    await tag.getByRole("button", { name: "Save the tag" }).click();
    await expect(page.getByRole("status")).toContainText("Tag saved");
    await page.goto(`/t/${shop.slug}/api-keys`);
    await page.getByLabel("Where shoppers find the shop").fill("https://refused.example");
    await page.getByRole("button", { name: "Save address" }).click();

    // The campaign on the shop's name, as it comes.
    await page.goto(`/t/${shop.slug}/ads/campaigns`);
    const brand = page.locator("[data-campaign=brand]");
    const form = brand.locator("[data-campaign-form]");
    await form.getByLabel("Countries").fill("FR");
    await form.getByRole("button", { name: "Prepare the campaign" }).click();
    await expect(page.getByRole("status")).toContainText("The campaign is prepared.");
    const proposalUrl = page.url().split("?")[0];
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(async () => {
      await page.goto(proposalUrl);
      await expect(page.locator("[data-proposal-status]")).toHaveAttribute("data-proposal-status", "failed", {
        timeout: 1000,
      });
    }).toPass({ timeout: 30_000 });
    await expect(page.locator("[data-proposal-outcome]")).toContainText(
      "Google Ads refused it: Merchant ID is either not found or not linked to the Google Ads customer.",
    );
    expect(google.adsCampaigns).toHaveLength(0);

    // Nothing was created: the page offers the form again.
    await expect(async () => {
      await page.goto(`/t/${shop.slug}/ads/campaigns`);
      await expect(form).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
  } finally {
    await google.stop();
  }
});
