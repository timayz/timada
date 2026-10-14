import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Store API keys: an owner creates one under /t/{slug}/api-keys (read-only
// unless store:write is ticked), sees the token exactly once, changes its
// scopes and revokes it; a member whose role lacks
// `api-keys:manage` cannot open the page.

test("an owner creates a key, sees its token once and revokes it", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Keys Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "API keys", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/api-keys$`));
  await expect(page.getByText("No key yet.")).toBeVisible();

  await page.getByLabel("Label").fill("Web storefront");
  await page.getByRole("button", { name: "Create key" }).click();
  await expect(page.getByRole("status")).toContainText("Key created.");
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_[A-Za-z0-9_-]{43}$/);
  const hint = token.slice(0, 11);

  // Shown once: a reload has no token, only the hint in the list (which
  // comes from the read model, a moment behind).
  const row = page.locator(`[data-api-key="${hint}"]`);
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-api-key-token]")).toHaveCount(0);
    await expect(row).toContainText("Web storefront", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(row.locator("[data-api-key-status]")).toHaveAttribute("data-api-key-status", "active");
  const scopes = row.locator("[data-api-key-scopes]");
  await expect(scopes).toHaveAttribute("data-api-key-scopes", "store:read");

  // Granted write, then refused a key with no scope at all.
  await row.getByRole("checkbox", { name: "store:write" }).check();
  await row.getByRole("button", { name: "Save scopes" }).click();
  await expect(page.getByRole("status")).toContainText("Scopes saved.");
  await expect(async () => {
    await page.reload();
    await expect(scopes).toHaveAttribute("data-api-key-scopes", "store:read store:write", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await row.getByRole("checkbox", { name: "store:read" }).uncheck();
  await row.getByRole("checkbox", { name: "store:write" }).uncheck();
  await row.getByRole("button", { name: "Save scopes" }).click();
  await expect(page).toHaveURL(/error=invalid_api_key_scopes/);

  // An empty label is refused.
  await page.getByLabel("Label").fill("   ");
  await page.getByRole("button", { name: "Create key" }).click();
  await expect(page).toHaveURL(/error=invalid_api_key_label/);

  await row.getByRole("button", { name: "Revoke" }).click();
  await expect(page.getByRole("status")).toContainText("Key revoked.");
  await expect(async () => {
    await page.reload();
    await expect(row.locator("[data-api-key-status]")).toHaveAttribute("data-api-key-status", "revoked", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(row.getByRole("button", { name: "Revoke" })).toHaveCount(0);
  await expect(row.getByRole("button", { name: "Save scopes" })).toHaveCount(0);
});

test("the keys page needs api-keys:manage", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Keys");
  await createTenant(owner, name);

  // A "viewer" role with no permission at all, and a member holding it.
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
  await expect(viewer.getByRole("link", { name: "API keys", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/api-keys`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});
