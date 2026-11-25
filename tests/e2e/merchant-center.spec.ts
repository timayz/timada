import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { FakeGoogle } from "./google";
import { createTenant, uniqueTenant } from "./tenants";

// What Merchant Center says of a shop's products, end to end against a
// stand-in for Google (google.ts): a shop that connected its Google account
// chooses which Merchant Center account takes its feed, and the account is
// read off the page: how many products Google accepts, still looks at or
// refuses, what it holds against the account, and the products to look at,
// each with Google's reason and the field it names. A product put right is
// read again when a member asks, and the product's own page says where it
// stands. A Cloud project Google does not know for Merchant Center says so
// in Google's words.

test("a shop chooses its Merchant Center account and reads where its products stand", async ({ page }) => {
  const google = new FakeGoogle();
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const shop = uniqueTenant("Merchant");
    await createTenant(page, shop.name);

    // A product of the shop, which one of the feed's items is.
    await page.goto(`/t/${shop.slug}/products`);
    await page.getByLabel("Name").fill("Arbre a chat");
    await page.getByRole("button", { name: "Create product" }).click();
    await expect(page.getByRole("status")).toContainText("Product created.");
    const productUrl = page.url().split("?")[0];
    const productId = new URL(productUrl).pathname.split("/").at(-1)!;
    // Nothing read yet: the product's page says nothing of Google.
    await expect(page.locator("[data-product-merchant]")).toHaveCount(0);

    google.merchantAccounts = [
      { accountId: "5550001", accountName: "Acme" },
      { accountId: "5550002", accountName: "Other shop" },
    ];
    google.merchantProducts = [
      {
        offerId: productId,
        ads: "disapproved",
        listings: "approved",
        issues: [
          {
            code: "image_too_small",
            attribute: "image_link",
            description: "Image too small",
            detail: "Use an image of at least 100 by 100 pixels",
          },
        ],
      },
      { offerId: "01OTHER", ads: "pending", listings: "pending" },
      { offerId: "01FINE", ads: "approved", listings: "approved" },
    ];
    google.merchantIssues = [
      {
        title: "Store ownership not verified",
        severity: "CRITICAL",
        detail: "Verify and claim your store's address.",
        documentationUri: "https://support.google.com/merchants/answer/176793",
      },
    ];

    // Nothing to read through yet.
    await page.getByRole("link", { name: "Merchant Center", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/merchant-center$`));
    await expect(page.locator("[data-merchant]")).toHaveAttribute("data-merchant", "not-connected");
    await page.locator("[data-merchant]").getByRole("link", { name: "Google account" }).click();

    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("status")).toContainText("Google account connected.");
    await page.goto(`/t/${shop.slug}/merchant-center`);
    await expect(page.locator("[data-merchant]")).toHaveAttribute("data-merchant", "no-account");

    // The accounts the grant reaches.
    await page.goto(`/t/${shop.slug}/google`);
    await expect(page.locator("[data-google-merchant-missing]")).toBeVisible();
    const accounts = page.getByRole("group", { name: "The Merchant Center accounts this Google account reaches" });
    await expect(accounts.getByRole("radio")).toHaveCount(2);
    await accounts.getByRole("radio", { name: /5550001/ }).check();
    await page.getByRole("button", { name: "Save the Merchant Center account" }).click();
    await expect(page.getByRole("status")).toContainText("Merchant Center account saved.");
    await expect(page.locator("[data-google-merchant]")).toContainText("5550001 · Acme");
    await expect(accounts).toHaveCount(0);

    // Read off the page: three products, one refused for Shopping ads.
    await page.goto(`/t/${shop.slug}/merchant-center`);
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-merchant-products]")).toHaveAttribute("data-merchant-products", "3", {
        timeout: 1000,
      });
    }).toPass({ timeout: 30_000 });
    await expect(page.locator("[data-merchant]")).toHaveAttribute("data-merchant", "read");
    await expect(page.locator("[data-merchant-all-accepted]")).toHaveAttribute("data-merchant-all-accepted", "no");
    const ads = page.locator('[data-merchant-counts="ads"]');
    await expect(ads.locator("[data-accepted]")).toHaveAttribute("data-accepted", "1");
    await expect(ads.locator("[data-pending]")).toHaveAttribute("data-pending", "1");
    await expect(ads.locator("[data-refused]")).toHaveAttribute("data-refused", "1");
    const listings = page.locator('[data-merchant-counts="listings"]');
    await expect(listings.locator("[data-accepted]")).toHaveAttribute("data-accepted", "2");
    await expect(listings.locator("[data-refused]")).toHaveAttribute("data-refused", "0");

    // What Google holds against the account.
    const held = page.locator("[data-merchant-account-issue]");
    await expect(held).toContainText("Nothing is shown: Store ownership not verified");
    await expect(held.getByRole("link", { name: "What Google says" })).toHaveAttribute(
      "href",
      "https://support.google.com/merchants/answer/176793",
    );

    // The products to look at, the refused one first, told for what it is
    // in the catalog, with Google's reason and the field it names.
    const problems = page.locator("[data-merchant-problem]");
    await expect(problems).toHaveCount(2);
    const refused = problems.nth(0);
    await expect(refused).toHaveAttribute("data-merchant-problem", productId);
    await expect(refused.getByRole("link", { name: "Arbre a chat" })).toHaveAttribute(
      "href",
      new RegExp(`/t/${shop.slug}/products/${productId}$`),
    );
    await expect(refused.locator("[data-ads]")).toHaveAttribute("data-ads", "refused");
    await expect(refused.locator("[data-listings]")).toHaveAttribute("data-listings", "accepted");
    await expect(refused.locator('[data-merchant-issue="image_too_small"]')).toContainText("Image too small");
    await expect(refused.locator("[data-merchant-field]")).toHaveText("image_link");
    await expect(refused).toContainText("Use an image of at least 100 by 100 pixels");
    await expect(problems.nth(1)).toHaveAttribute("data-merchant-problem", "01OTHER");
    await expect(problems.nth(1).locator("[data-ads]")).toHaveAttribute("data-ads", "pending");

    // The product's own page says where Google has it.
    await page.goto(productUrl);
    await expect(page.locator("[data-product-merchant]")).toHaveAttribute("data-product-merchant", "refused");
    await expect(page.locator("[data-product-merchant]")).toContainText("Shopping ads: Refused");
    await page.locator("[data-product-merchant]").click();
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/merchant-center$`));

    // Put right at Google: a member asks for another read rather than wait
    // for tomorrow's.
    google.merchantProducts = [
      { offerId: productId, ads: "approved", listings: "approved" },
      { offerId: "01OTHER", ads: "approved", listings: "approved" },
      { offerId: "01FINE", ads: "approved", listings: "approved" },
    ];
    google.merchantIssues = [];
    const before = google.merchantReads;
    await page.getByRole("button", { name: "Read now" }).click();
    await expect(page.getByRole("status")).toContainText("Merchant Center read: 3 products.");
    expect(google.merchantReads).toBe(before + 1);
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-merchant-all-accepted]")).toHaveAttribute("data-merchant-all-accepted", "yes", {
        timeout: 1000,
      });
    }).toPass({ timeout: 30_000 });
    await expect(page.locator("[data-merchant-problem]")).toHaveCount(0);
    await expect(page.locator("[data-merchant-account-issue]")).toHaveCount(0);
    await expect(page.getByText("Google holds nothing against any product.")).toBeVisible();
    await page.goto(productUrl);
    await expect(page.locator("[data-product-merchant]")).toHaveAttribute("data-product-merchant", "accepted");
  } finally {
    await google.stop();
  }
});

test("a project Google does not know for Merchant Center says so", async ({ page }) => {
  const google = new FakeGoogle();
  google.merchantUnregistered = true;
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const shop = uniqueTenant("Merchant");
    await createTenant(page, shop.name);
    await page.goto(`/t/${shop.slug}/google`);
    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("status")).toContainText("Google account connected.");
    // Not a grant to renew: the operator has to register the project.
    await expect(page.locator("[data-google-merchant-fault]")).toContainText(
      "is not registered with the merchant account",
    );
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "connected");
  } finally {
    await google.stop();
  }
});
