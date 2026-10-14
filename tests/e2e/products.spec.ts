import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// The catalog: an owner creates a product under /t/{slug}/products, edits it
// on its page, uploads its pictures, publishes and archives it; the list (a
// read model, a moment behind) follows; a member whose role lacks
// `products:read` cannot open the pages.

/// A 1x1 PNG, as the bytes a browser posts when a file is chosen.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

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

test("an owner uploads, describes, reorders and removes a product's pictures", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Picture Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Products", exact: true }).click();
  await page.getByLabel("Name").fill("Blue Mug");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const productUrl = page.url().split("?")[0];
  await expect(page.getByText("No picture yet.")).toBeVisible();

  // Anything that is not a picture is refused, whatever it is called.
  await page.getByLabel("Add a picture").setInputFiles({
    name: "mug.png",
    mimeType: "image/png",
    buffer: Buffer.from("this is not a picture at all"),
  });
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page).toHaveURL(/error=unsupported_image/);
  await expect(page.getByRole("alert")).toContainText("JPEG, PNG, WebP or GIF");

  // A real one is taken, shown, and served from the picture address.
  await page.getByLabel("Add a picture").setInputFiles({ name: "front.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill("A blue mug, seen from the front");
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
  const first = page.locator("[data-image]").first();
  await expect(first.locator("img")).toHaveAttribute("alt", "A blue mug, seen from the front");
  await expect(first.locator("[data-image-first]")).toBeVisible();
  const src = await first.locator("img").getAttribute("src");
  expect(src).toMatch(/\/media\/[0-9A-Z]{26}\/[0-9A-Z]{26}\.png$/);
  const served = await request.get(src!);
  expect(served.status()).toBe(200);
  expect(served.headers()["content-type"]).toContain("image/png");
  expect(served.headers()["cache-control"]).toContain("immutable");

  // A second one goes last, and the two can swap.
  await page.getByLabel("Add a picture").setInputFiles({ name: "side.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill("The same mug from the side");
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
  await expect(page.locator("[data-image]")).toHaveCount(2);
  const second = page.locator("[data-image]").nth(1);
  await expect(second.locator("img")).toHaveAttribute("alt", "The same mug from the side");
  const secondId = await second.getAttribute("data-image");
  await second.getByRole("button", { name: "Move earlier" }).click();
  await expect(page.getByRole("status")).toContainText("Gallery reordered.");
  await expect(page.locator("[data-image]").first()).toHaveAttribute("data-image", secondId!);

  // The description of one picture changes on its own.
  await page.locator("[data-image]").first().getByLabel("What it shows").fill("Now the first one");
  await page.locator("[data-image]").first().getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Picture description saved.");
  await expect(page.locator("[data-image]").first().locator("img")).toHaveAttribute("alt", "Now the first one");

  // Removing takes the file with it.
  await page.locator("[data-image]").first().getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("status")).toContainText("Picture removed.");
  await expect(page.locator("[data-image]")).toHaveCount(1);
  await expect(page.locator("[data-image]").first().locator("img")).toHaveAttribute("src", src!);

  // An archived product keeps the gallery it had, with no way to change it.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Archive" }).click();
  await expect(page.getByRole("status")).toContainText("Product archived.");
  await expect(page.locator("[data-image]")).toHaveCount(1);
  await expect(page.getByLabel("Add a picture")).toHaveCount(0);
  await expect(page.locator("[data-image]").first().getByRole("button", { name: "Remove" })).toHaveCount(0);
});
