import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// An assistant's way into the dashboard: a user makes an agent key for one
// shop with two permissions, sees its token once, and an MCP client that
// presents it reads the shop and prepares a change, which waits as a
// proposal for a person. The page shows what the key asked for; withdrawn,
// the key is turned away. config/e2e.toml turns the server on.

test("a user makes an agent key, an assistant reads and prepares through it, and it is withdrawn", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("agent"));
  const shop = uniqueTenant("Agent");
  await createTenant(page, shop.name);
  await page.goto(`/t/${shop.slug}/products`);
  await page.getByLabel("Name").fill("Gants");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const productId = new URL(page.url()).pathname.split("/").at(-1)!;

  const ask = (token: string | null, body: unknown) =>
    page.request.post("/mcp", {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
      data: body,
    });
  const call = async (token: string, name: string, args: unknown) => {
    const answer = await ask(token, { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } });
    expect(answer.status()).toBe(200);
    const result = (await answer.json()).result;
    return { refused: result.isError as boolean, text: result.content[0].text as string };
  };
  // Without a key the server says nothing.
  expect((await ask(null, { jsonrpc: "2.0", id: 1, method: "ping" })).status()).toBe(401);

  // From the account page to the keys.
  await page.goto("/auth/account");
  await page.getByRole("link", { name: "Manage your agent keys" }).click();
  await expect(page).toHaveURL(/\/auth\/agent-keys$/);
  await expect(page.getByText("No key yet.")).toBeVisible();
  await expect(page.locator("[data-mcp-address]")).toHaveText("/mcp");

  // The shops come from a read model, which trails the shop's creation.
  const form = page.locator("[data-agent-key-form]");
  await expect(async () => {
    await page.reload();
    await expect(form).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  // Nothing ticked is no key.
  await form.getByLabel("Label").fill("Claude");
  await form.getByRole("button", { name: "Make the key" }).click();
  await expect(page.getByRole("alert")).toContainText("A key covers one to twenty shops");

  await form.getByLabel("Label").fill("Claude");
  const grants = form.locator("[data-agent-key-shop]").filter({ hasText: shop.name });
  await grants.getByRole("checkbox", { name: /^products:read/ }).check();
  await grants.getByRole("checkbox", { name: /^proposals:prepare/ }).check();
  await form.getByRole("button", { name: "Make the key" }).click();
  await expect(page.getByRole("status")).toContainText("Key made.");
  const token = (await page.locator("[data-agent-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^mk_/);
  const key = page.locator("[data-agent-key]");
  await expect(key).toHaveAttribute("data-agent-key-state", "active");
  await expect(key.locator("[data-agent-key-covers]")).toContainText(`${shop.name}: products:read, proposals:prepare`);
  await expect(key).toContainText("never used");
  // Shown once.
  await page.reload();
  await expect(page.locator("[data-agent-key-token]")).toHaveCount(0);

  // The assistant says hello, and reads what the key covers.
  const hello = await ask(token, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } },
  });
  expect(hello.status()).toBe(200);
  expect((await hello.json()).result.serverInfo.name).toBe("timada");
  expect((await ask(token, { jsonrpc: "2.0", method: "notifications/initialized" })).status()).toBe(202);
  const listed = await (await ask(token, { jsonrpc: "2.0", id: 2, method: "tools/list" })).json();
  expect(listed.result.tools.map((tool: { name: string }) => tool.name)).toContain("propose_product_change");

  const shops = await call(token, "list_shops", {});
  expect(JSON.parse(shops.text).shops).toEqual([
    { slug: shop.slug, name: shop.name, permissions: ["products:read", "proposals:prepare"] },
  ]);
  const product = await call(token, "get_product", { shop: shop.slug, product_id: productId });
  expect(JSON.parse(product.text).name).toBe("Gants");
  // The sales were not given to the key.
  const sales = await call(token, "sales_report", { shop: shop.slug });
  expect(sales.refused).toBe(true);
  expect(sales.text).toContain("does not cover orders:read");

  // A change it prepares waits for a person, under the key's name.
  const prepared = await call(token, "propose_product_change", {
    shop: shop.slug,
    product_id: productId,
    name: "Gants moto",
    reason: "The keyword has more searches.",
  });
  expect(prepared.refused).toBe(false);
  expect(JSON.parse(prepared.text).status).toBe("waiting");
  await expect(async () => {
    await page.goto(`/t/${shop.slug}/proposals`);
    await expect(page.locator("[data-proposal-status=waiting]").first()).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("main")).toContainText("Agent Claude");
  await expect(page.locator("main")).toContainText("Gants moto");
  await page.goto(`/t/${shop.slug}/products/${productId}`);
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Gants");
  await expect(page.getByRole("heading", { level: 1 })).not.toContainText("Gants moto");

  // What it asked for is on its page.
  await page.goto("/auth/agent-keys");
  await expect(key.locator("[data-agent-key-calls]")).toHaveAttribute("data-agent-key-calls", "4");
  await key.getByText("Its last calls").click();
  await expect(key.locator("[data-agent-key-log]")).toContainText("propose_product_change");
  await expect(key.locator("[data-agent-key-log]")).toContainText("sales_report");
  await expect(key.locator("[data-agent-key-log]")).toContainText("refused");

  // Withdrawn: turned away.
  await key.getByRole("button", { name: "Withdraw" }).click();
  await expect(page.getByRole("status")).toContainText("Key withdrawn.");
  await expect(key).toHaveAttribute("data-agent-key-state", "withdrawn");
  expect((await ask(token, { jsonrpc: "2.0", id: 3, method: "ping" })).status()).toBe(401);
});
