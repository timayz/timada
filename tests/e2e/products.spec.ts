import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// The catalog: an owner creates a product under /t/{slug}/products, edits it
// on its page, publishes and archives it; the list (a read model, a moment
// behind) follows; a member whose role lacks `products:read` cannot open the
// pages.

test("an owner creates, edits, publishes and archives a product", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Catalog Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Products", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/products$`));
  await expect(page.getByText("No product yet.")).toBeVisible();

  // Create: lands on the product page, a draft without a price.
  await page.getByLabel("Name").fill("Blue Mug");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/products/[0-9A-Z]{26}\\?`));
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "draft");
  await expect(page.locator("[data-product-price]")).toContainText("Not priced yet");
  const productUrl = page.url().split("?")[0];

  // Publishing needs a price; a bad amount is refused.
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page).toHaveURL(/error=product_not_priced/);
  await page.getByLabel("Price").fill("abc");
  await page.getByRole("button", { name: "Set price" }).click();
  await expect(page).toHaveURL(/error=invalid_amount/);

  // Edit everything, then publish.
  await page.getByLabel("Price").fill("12.50");
  await page.getByLabel("Currency").selectOption("EUR");
  await page.getByRole("button", { name: "Set price" }).click();
  await expect(page.getByRole("status")).toContainText("Price set.");
  await expect(page.locator("[data-product-price]")).toContainText("12.50 EUR");
  await page.getByLabel("Description").fill("Holds 50 cl of anything hot.");
  await page.getByRole("button", { name: "Save description" }).click();
  await expect(page.locator("[data-product-description]")).toContainText("Holds 50 cl");
  await page.getByLabel("Name").fill("Big Blue Mug");
  await page.getByRole("button", { name: "Rename" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Big Blue Mug");
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.getByRole("status")).toContainText("Product published.");
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");

  // The list comes from the read model, a moment behind.
  const row = page.locator("[data-product=blue-mug]");
  await expect(async () => {
    await page.goto(`/t/${slug}/products`);
    await expect(row).toHaveAttribute("data-product-status", "published", { timeout: 1000 });
    await expect(row).toContainText("Big Blue Mug", { timeout: 1000 });
    await expect(row).toContainText("12.50 EUR", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.getByLabel("Search").fill("nothing-like-it");
  await page.getByRole("button", { name: "Search" }).click();
  await expect(page.getByText("No product yet.")).toBeVisible();

  // Archive: no more edits.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Archive" }).click();
  await expect(page.getByRole("status")).toContainText("Product archived.");
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "archived");
  await expect(page.getByRole("button", { name: "Rename" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Archive" })).toHaveCount(0);
});

test("the products pages need products:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Catalog");
  await createTenant(owner, name);

  // A "viewer" role with no permission at all.
  await owner.getByRole("link", { name: "Roles", exact: true }).click();
  const define = owner.locator("form", { has: owner.getByRole("button", { name: "Define role" }) });
  await define.getByLabel("Role name").fill("viewer");
  await define.getByLabel("Label").fill("Viewer");
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role defined.");
  const viewerEmail = uniqueEmail("viewer");
  await owner.getByRole("link", { name: "Members", exact: true }).click();
  await invite(owner, viewerEmail, "Viewer");
  const link = await inviteLink(request, viewerEmail);

  const viewerContext = await browser.newContext();
  const viewer = await viewerContext.newPage();
  await registerWithEmail(viewer, viewerEmail);
  await viewer.goto(link);
  await viewer.getByRole("button", { name: "Accept the invitation" }).click();
  await expect(viewer).toHaveURL(new RegExp(`/t/${slug}`));
  await expect(viewer.getByRole("link", { name: "Products", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/products`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});
