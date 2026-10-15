import { expect, test, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

/// Picks the option of a `<select>` whose text contains `text`: the category
/// menus prefix a sub-category with its indent, so the label is not the name.
async function selectByText(page: Page, label: string, text: string) {
  const select = page.getByLabel(label);
  const value = await select.locator("option", { hasText: text }).first().getAttribute("value");
  await select.selectOption(value ?? "");
}

/// Opens the category list and waits for `slug` to be in the tree: the list
/// and the parent menu come from a read model, a moment behind the write.
async function openCategories(page: Page, tenant: string, slug?: string) {
  await expect(async () => {
    await page.goto(`/t/${tenant}/categories`);
    if (slug) {
      await expect(page.locator(`[data-category=${slug}]`)).toBeVisible({ timeout: 1000 });
    }
  }).toPass({ timeout: 15_000 });
}

// The category tree: an owner builds it under /t/{slug}/categories, files a
// product under a category from the product page, and the tree (a read model,
// a moment behind) follows with its counts. A member whose role lacks
// `products:read` cannot open the pages.

test("an owner builds a category tree and files a product under it", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Tree Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Categories", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/categories$`));
  await expect(page.getByText("No category yet.")).toBeVisible();

  // A top-level category, then one under it.
  await page.getByLabel("Name").fill("Bedding");
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/categories/[0-9A-Z]{26}\\?`));
  const beddingUrl = page.url().split("?")[0];
  await expect(page.locator("[data-breadcrumb]")).toContainText("Bedding");

  await openCategories(page, slug, "bedding");
  await page.getByLabel("Name").fill("Mats");
  await selectByText(page, "Parent", "Bedding");
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");
  const matsUrl = page.url().split("?")[0];
  // The breadcrumb reads top-level first.
  await expect(page.locator("[data-breadcrumb]")).toContainText("Bedding");
  await expect(page.locator("[data-breadcrumb]")).toContainText("Mats");

  // Edit it: rename, describe, position.
  await page.getByLabel("Name").fill("Cat mats");
  await page.getByRole("button", { name: "Rename" }).click();
  await expect(page.getByRole("status")).toContainText("Category renamed.");
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Cat mats");
  await page.getByLabel("Description").fill("Where a cat naps.");
  await page.getByRole("button", { name: "Save description" }).click();
  await expect(page.locator("[data-category-description]")).toContainText("Where a cat naps");
  await page.getByLabel("Position among its siblings").fill("2");
  await page.getByRole("button", { name: "Set position" }).click();
  await expect(page.getByRole("status")).toContainText("Position set.");

  // Out of the shop's menu, though its page stays.
  await expect(page.locator("[data-category-menu]")).toHaveAttribute("data-category-menu", "shown");
  await page.getByLabel("Show in the shop menu").uncheck();
  await page.getByRole("button", { name: "Save menu visibility" }).click();
  await expect(page.getByRole("status")).toContainText("Hidden from the menu.");
  await expect(page.locator("[data-category-menu]")).toHaveAttribute("data-category-menu", "hidden");
  await expect(page.getByLabel("Show in the shop menu")).not.toBeChecked();
  await expect(async () => {
    await page.goto(`/t/${slug}/categories`);
    await expect(page.locator("[data-category=mats] [data-hidden-from-menu]")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("[data-category=bedding] [data-hidden-from-menu]")).toHaveCount(0);
  await page.goto(matsUrl);
  await page.getByLabel("Show in the shop menu").check();
  await page.getByRole("button", { name: "Save menu visibility" }).click();
  await expect(page.getByRole("status")).toContainText("Shown in the menu.");

  // The tree shows both, the child indented, with its own count.
  await expect(async () => {
    await page.goto(`/t/${slug}/categories`);
    await expect(page.locator("[data-category=bedding]")).toHaveAttribute("data-category-depth", "0", { timeout: 1000 });
    await expect(page.locator("[data-category=mats]")).toHaveAttribute("data-category-depth", "1", { timeout: 1000 });
    await expect(page.locator("[data-category=mats]")).toContainText("Cat mats", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  // A product joins the category from its own page.
  await page.getByRole("link", { name: "Products", exact: true }).click();
  await page.getByLabel("Name").fill("Cooling Mat");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const productUrl = page.url().split("?")[0];
  await expect(async () => {
    await page.goto(productUrl);
    await expect(page.getByLabel("Category")).toContainText("Cat mats", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await selectByText(page, "Category", "Cat mats");
  await page.getByRole("button", { name: "Add to category" }).click();
  await expect(page.getByRole("status")).toContainText("Added to the category.");
  await expect(page.locator("[data-membership=mats]")).toContainText("Cat mats");

  // The count reaches the tree. A product in a sub-category counts for its
  // parent too, because the parent's page in the shop lists it.
  await expect(async () => {
    await page.goto(`/t/${slug}/categories`);
    await expect(page.locator("[data-category=mats] [data-category-count]")).toHaveText("1", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  const bedding = page.locator("[data-category=bedding]");
  await expect(bedding.locator("[data-category-count]")).toHaveText("1");
  await expect(bedding.locator("[data-category-direct]")).toHaveAttribute("data-category-direct", "0");
  await expect(bedding).toContainText("0 filed here");
  // A leaf says one number, not two.
  await expect(page.locator("[data-category=mats] [data-category-direct]")).toHaveCount(0);

  // The same two numbers on each category's own page.
  await page.goto(matsUrl);
  await expect(page.locator("[data-category-products]")).toContainText("1 product");
  await expect(page.locator("[data-category-direct]")).toContainText("1 product");
  await page.goto(beddingUrl);
  await expect(page.locator("[data-category-products]")).toContainText("1 product");
  await expect(page.locator("[data-category-products]")).toContainText("counting its sub-categories");
  await expect(page.locator("[data-category-direct]")).toContainText("0 products");

  // The products list filters by category.
  await expect(async () => {
    await page.goto(`/t/${slug}/products`);
    await expect(page.getByLabel("Category")).toContainText("Cat mats", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await selectByText(page, "Category", "Cat mats");
  await page.getByRole("button", { name: "Search" }).click();
  await expect(page.locator("[data-product=cooling-mat]")).toBeVisible();

  // Leaving the category is immediate on the product page.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("status")).toContainText("Removed from the category.");
  await expect(page.locator("[data-membership=mats]")).toHaveCount(0);

  // A parent with a live child is not archived.
  await page.goto(beddingUrl);
  await expect(page.locator("[data-category-children]")).toHaveText("1");
  await expect(page.getByRole("button", { name: "Archive" })).toHaveCount(0);
  await expect(page.getByText("Archive or move its sub-categories")).toBeVisible();

  // The child moves to the top level, and then the parent can go.
  await page.goto(matsUrl);
  await selectByText(page, "Parent", "Top level");
  await page.getByRole("button", { name: "Move" }).click();
  await expect(page.getByRole("status")).toContainText("Category moved.");
  await page.goto(beddingUrl);
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-category-children]")).toHaveText("0", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.getByRole("button", { name: "Archive" }).click();
  await expect(page.getByRole("status")).toContainText("Category archived.");
  await expect(page.locator("[data-category-archived]")).toBeVisible();
  await expect(page.getByRole("button", { name: "Rename" })).toHaveCount(0);
});

test("a category cannot be moved under its own sub-category", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Cycle Shop");
  await createTenant(page, name);

  await page.goto(`/t/${slug}/categories`);
  await page.getByLabel("Name").fill("Bedding");
  await page.getByRole("button", { name: "Create category" }).click();
  const beddingUrl = page.url().split("?")[0];
  const beddingId = beddingUrl.split("/").pop()!;
  await openCategories(page, slug, "bedding");
  await page.getByLabel("Name").fill("Mats");
  await selectByText(page, "Parent", "Bedding");
  await page.getByRole("button", { name: "Create category" }).click();
  const matsId = page.url().split("?")[0].split("/").pop()!;

  // The form leaves out a parent that would make a loop, so post it directly.
  const refused = await page.request.post(`/t/${slug}/categories/${beddingId}/move-to`, {
    form: { parent_id: matsId },
    maxRedirects: 0,
  });
  const location = refused.headers()["location"];
  expect(location).toContain("error=category_cycle");
  await page.goto(location);
  await expect(page.getByRole("alert")).toContainText("sub-categories");
  // And the move did not happen.
  await page.goto(beddingUrl);
  await expect(page.locator("[data-breadcrumb]")).not.toContainText("Mats");
});

test("the category pages need products:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Tree");
  await createTenant(owner, name);

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
  await expect(viewer.getByRole("link", { name: "Categories", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/categories`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});
