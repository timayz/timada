import { expect, type APIRequestContext, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

// Shared between the specs of the example storefronts: a merchant mints an
// API key and publishes products in the dashboard, then an example binary
// (a separate process built on timada-storefront) is booted with the key
// and followed until it has synced the catalog.

/// The store API address the example binaries connect to.
export const STORE_ADDR = process.env.E2E_STORE_ADDR ?? "127.0.0.1:4321";

/// A 1x1 PNG, as the bytes a browser posts when a file is chosen.
export const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

/// What `mintKey` reads back: the token and the addresses the example needs.
export type StoreKey = { token: string; mediaUrl: string; apiUrl?: string };

/// Mints a key on the tenant's page and reads it, with the address the
/// tenant's pictures are served from and the dashboard's own. `sell` adds
/// store:checkout to the default read scope.
export async function mintKey(
  page: Page,
  slug: string,
  sell = false,
): Promise<{ token: string; mediaUrl: string; apiUrl: string }> {
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Label").fill("Example storefront");
  if (sell) {
    await page.getByRole("checkbox", { name: "store:checkout" }).first().check();
  }
  await page.getByRole("button", { name: "Create key" }).click();
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_/);
  const mediaUrl = (await page.locator("[data-media-url]").textContent())?.trim() ?? "";
  expect(mediaUrl).toContain("/media/");
  const apiUrl = (await page.locator("[data-api-url]").textContent())?.trim() ?? "";
  expect(apiUrl).toMatch(/^https?:\/\//);
  return { token, mediaUrl, apiUrl };
}

/// Creates a published product priced at `amount`, filed under the category
/// whose menu text contains `category` when one is given.
export async function publishProduct(page: Page, slug: string, name: string, amount: string, category?: string) {
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

/// An example storefront binary from `target/debug`, run as a child process
/// on `port` with a throwaway database. One instance per spec; `stop` it
/// after each test.
export class ExampleApp {
  /// Where the example serves, for the browser and the API client.
  readonly url: string;
  private child: ChildProcess | undefined;
  private dir: string | undefined;

  constructor(
    readonly binary: string,
    readonly port: number,
  ) {
    this.url = `http://127.0.0.1:${port}`;
  }

  /// Boots the example with `token` and waits for it to have synced at
  /// least `products` products.
  async start(request: APIRequestContext, { token, mediaUrl, apiUrl }: StoreKey, products: number): Promise<void> {
    this.dir = fs.mkdtempSync(path.join(os.tmpdir(), `timada-${this.binary}-`));
    this.child = spawn(path.resolve(`target/debug/${this.binary}`), [], {
      env: {
        ...process.env,
        STORE_ADDR,
        STORE_TOKEN: token,
        STORE_MEDIA_URL: mediaUrl,
        ...(apiUrl ? { STORE_API_URL: apiUrl } : {}),
        STORE_DB: path.join(this.dir, "storefront.db"),
        HOST: "127.0.0.1",
        PORT: String(this.port),
      },
      stdio: "inherit",
    });
    await expect(async () => {
      const response = await request.get(`${this.url}/store/status`);
      expect(response.ok()).toBe(true);
      const status = (await response.json()) as { connected: boolean; products: number };
      expect(status.connected).toBe(true);
      expect(status.products).toBeGreaterThanOrEqual(products);
    }).toPass({ timeout: 30_000 });
  }

  /// Kills the process and removes its database; a no-op when not started.
  stop(): void {
    this.child?.kill();
    this.child = undefined;
    if (this.dir) {
      fs.rmSync(this.dir, { recursive: true, force: true });
      this.dir = undefined;
    }
  }
}
