import { expect, test } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// The headless storefront end to end: a merchant publishes a product and
// mints an API key in the dashboard; the example storefront (a separate
// process built on timada-storefront) connects to the store API with the
// key, shows the product, and follows the catalog live.

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

  // A key, shown once.
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Label").fill("Example storefront");
  await page.getByRole("button", { name: "Create key" }).click();
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_/);

  // The storefront boots with it and syncs the catalog into its own file.
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
    expect(status.products).toBeGreaterThan(0);
  }).toPass({ timeout: 30_000 });

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
