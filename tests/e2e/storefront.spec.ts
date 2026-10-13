import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// The headless storefront end to end: a merchant publishes a product and
// mints an API key in the dashboard; the example storefront (a separate
// process built on timada-storefront) connects to the store API with the
// key, shows the product, follows the catalog live, and browses the
// categories the products are filed under.

const STORE_ADDR = process.env.E2E_STORE_ADDR ?? "127.0.0.1:4321";
const STOREFRONT_PORT = Number(process.env.E2E_STOREFRONT_PORT ?? 3456);
const STOREFRONT = `http://127.0.0.1:${STOREFRONT_PORT}`;

let child: ChildProcess | undefined;
let dir: string | undefined;

test.afterEach(async () => {
  child?.kill();
  child = undefined;
  if (dir) {
    fs.rmSync(dir, { recursive: true, force: true });
    dir = undefined;
  }
});

/// Mints a key on the tenant's page and shows it once.
async function mintKey(page: Page, slug: string): Promise<string> {
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Label").fill("Example storefront");
  await page.getByRole("button", { name: "Create key" }).click();
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_/);
  return token;
}

/// Boots the example storefront with `token` and waits for it to have synced
/// at least `products` products.
async function startStorefront(request: APIRequestContext, token: string, products: number) {
  dir = fs.mkdtempSync(path.join(os.tmpdir(), "timada-storefront-"));
  child = spawn(path.resolve("target/debug/storefront"), [], {
    env: {
      ...process.env,
      STORE_ADDR,
      STORE_TOKEN: token,
      STORE_DB: path.join(dir, "storefront.db"),
      HOST: "127.0.0.1",
      PORT: String(STOREFRONT_PORT),
    },
    stdio: "inherit",
  });
  await expect(async () => {
    const response = await request.get(`${STOREFRONT}/store/status`);
    expect(response.ok()).toBe(true);
    const status = (await response.json()) as { connected: boolean; products: number };
    expect(status.connected).toBe(true);
    expect(status.products).toBeGreaterThanOrEqual(products);
  }).toPass({ timeout: 30_000 });
}

/// Creates a published product priced at `amount`, filed under the category
/// whose menu text contains `category` when one is given.
async function publishProduct(page: Page, slug: string, name: string, amount: string, category?: string) {
  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const url = page.url().split("?")[0];
  await page.getByLabel("Price").fill(amount);
  await page.getByRole("button", { name: "Set price" }).click();
  if (category) {
    await expect(async () => {
      await page.goto(url);
      await expect(page.getByLabel("Category")).toContainText(category, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    const select = page.getByLabel("Category");
    const value = await select.locator("option", { hasText: category }).first().getAttribute("value");
    await select.selectOption(value ?? "");
    await page.getByRole("button", { name: "Add to category" }).click();
    await expect(page.getByRole("status")).toContainText("Added to the category.");
  }
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");
  return url;
}

test("a storefront serves the tenant's published catalog with its key", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Mug Shop");
  await createTenant(page, name);

  // A published product.
  await page.getByRole("link", { name: "Products", exact: true }).click();
  await page.getByLabel("Name").fill("Blue Mug");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  await page.getByLabel("Price").fill("12.50");
  await page.getByRole("button", { name: "Set price" }).click();
  await page.getByLabel("Description").fill("Holds 50 cl of anything hot.");
  await page.getByRole("button", { name: "Save description" }).click();
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");
  const productUrl = page.url().split("?")[0];

  // A key, shown once; the storefront boots with it and syncs the catalog
  // into its own file.
  const token = await mintKey(page, slug);
  await startStorefront(request, token, 1);

  await page.goto(`${STOREFRONT}/`);
  const card = page.locator("[data-product=blue-mug]");
  await expect(card).toContainText("Blue Mug");
  await expect(card.locator("[data-price]")).toHaveText("12.50 EUR");
  await card.getByRole("link", { name: "Blue Mug" }).click();
  await expect(page).toHaveURL(`${STOREFRONT}/p/blue-mug`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Blue Mug");
  await expect(page.locator(".description")).toContainText("Holds 50 cl");
  expect((await page.goto(`${STOREFRONT}/p/nothing-here`))?.status()).toBe(404);

  // Taken off sale in the dashboard, gone from the storefront.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Unpublish" }).click();
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "draft");
  await expect(async () => {
    await page.goto(`${STOREFRONT}/`);
    await expect(page.getByText("Nothing on sale right now.")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
});

test("a storefront browses the categories its products are filed under", async ({ page, request }) => {
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
    await expect(page.locator("[data-category=bedding]")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.getByLabel("Name").fill("Mats");
  const parent = page.getByLabel("Parent");
  await parent.selectOption(
    (await parent.locator("option", { hasText: "Bedding" }).first().getAttribute("value")) ?? "",
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

  const token = await mintKey(page, slug);
  await startStorefront(request, token, 3);

  // The menu carries the tree with its counts; Bedding counts its subtree.
  await expect(async () => {
    await page.goto(`${STOREFRONT}/`);
    await expect(page.locator("nav.sections [data-category=bedding]")).toContainText("Bedding (2)", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("nav.sections [data-category=mats]")).toContainText("Mats (2)");
  await expect(page.locator("nav.sections [data-category=toys]")).toContainText("Toys (1)");

  // A top-level category lists what is under it, newest first by default.
  await page.locator("nav.sections [data-category=bedding]").click();
  await expect(page).toHaveURL(`${STOREFRONT}/c/bedding`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Bedding");
  await expect(page.locator("[data-subcategory=mats]")).toContainText("Mats (2)");
  await expect(page.locator("ul.catalog li")).toHaveCount(2);
  await expect(page.locator("[data-product=feather-wand]")).toHaveCount(0);

  // Sorting is a plain GET form.
  await page.getByLabel("Sort by").selectOption("price_asc");
  await page.getByRole("button", { name: "Sort" }).click();
  await expect(page).toHaveURL(/sort=price_asc/);
  await expect(page.locator("ul.catalog li").first()).toContainText("Wool Mat");
  await page.getByLabel("Sort by").selectOption("price_desc");
  await page.getByRole("button", { name: "Sort" }).click();
  await expect(page.locator("ul.catalog li").first()).toContainText("Cooling Mat");

  // A sub-category page carries a breadcrumb up to its parent.
  await page.goto(`${STOREFRONT}/c/mats`);
  await expect(page.locator("[data-crumbs]")).toContainText("Bedding");
  await expect(page.locator("[data-crumbs]")).toContainText("Mats");
  expect((await page.goto(`${STOREFRONT}/c/nothing-here`))?.status()).toBe(404);

  // A product page carries its category and what else is in it.
  await page.goto(`${STOREFRONT}/p/cooling-mat`);
  await expect(page.locator("[data-crumbs]")).toContainText("Mats");
  const related = page.locator("section.related");
  await expect(related).toContainText("You may also like");
  await expect(related.locator("[data-product=wool-mat]")).toBeVisible();
  await expect(related.locator("[data-product=cooling-mat]")).toHaveCount(0);
});
