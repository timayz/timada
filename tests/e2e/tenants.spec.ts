import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { ADMIN_STATE } from "./fixtures";
import { invite } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Tenants: self-service creation, the tenant's own page, and the platform
// administration (status, suspend, resume).

test("a user creates a tenant, owns it and renames it", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  await expect(page.getByText("You do not belong to any tenant yet.")).toBeVisible();

  const { name, slug } = uniqueTenant("Acme Shop");
  await page.getByRole("link", { name: "Create a tenant" }).click();
  await expect(page).toHaveURL(/\/tenants\/new$/);
  await expect(page.getByLabel("Slug")).toHaveCount(0);
  await page.getByLabel("Name").fill(`  ${name.toUpperCase()}  `);
  await page.getByRole("button", { name: "Create tenant" }).click();

  await expect(page).toHaveURL(new RegExp(`/t/${slug}\\?`));
  await expect(page.getByRole("status")).toContainText("Tenant created.");
  await expect(page.getByRole("heading", { name: name.toUpperCase() })).toBeVisible();
  await expect(page.getByText("/t/" + slug)).toBeVisible();
  await expect(page.locator("dd", { hasText: "owner" })).toBeVisible();
  await expect(page.locator("[data-tenant-status]")).toHaveAttribute("data-tenant-status", "running");

  // Owners rename.
  await page.getByLabel("Name").fill("Acme Corporation");
  await page.getByRole("button", { name: "Rename" }).click();
  await expect(page.getByRole("status")).toContainText("Tenant renamed.");
  await expect(page.getByRole("heading", { name: "Acme Corporation" })).toBeVisible();

  // The home page lists it (read model, so it may lag a moment).
  await page.goto("/");
  await expect(async () => {
    await page.reload();
    await expect(page.getByRole("link", { name: "Acme Corporation" })).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  // The same name again: the slug is taken, so the new tenant gets a suffix.
  await createTenant(page, name);
  await expect(page).toHaveURL(new RegExp(`/t/${slug}-[a-z0-9]{5}\\?`));
  await expect(page.getByRole("heading", { name })).toBeVisible();

  // An empty name is refused and the form comes back.
  await page.goto("/tenants/new");
  await page.getByLabel("Name").fill("   ");
  await page.getByRole("button", { name: "Create tenant" }).click();
  await expect(page).toHaveURL(/\/tenants\/new\?error=invalid_name/);
  await expect(page.getByRole("alert").first()).toContainText("between 1 and 80");
});

test("only members see a tenant", async ({ browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Private");
  await createTenant(owner, name);
  await expect(owner).toHaveURL(new RegExp(`/t/${slug}`));

  const strangerContext = await browser.newContext();
  const stranger = await strangerContext.newPage();
  await registerWithEmail(stranger, uniqueEmail("stranger"));
  expect((await stranger.goto(`/t/${slug}`))?.status()).toBe(403);
  expect((await stranger.goto(`/t/${uniqueTenant("nobody").slug}`))?.status()).toBe(404);
  expect((await stranger.goto("/admin/tenants"))?.status()).toBe(403);

  await ownerContext.close();
  await strangerContext.close();
});

test("the administrator sees a tenant running, suspends and resumes it", async ({ browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Quiet Shop");
  await createTenant(owner, name);
  await expect(owner).toHaveURL(new RegExp(`/t/${slug}`));
  await owner.goto(`/t/${slug}/members`);
  await invite(owner, uniqueEmail("invitee"), "owner");

  const adminContext = await browser.newContext({ storageState: ADMIN_STATE });
  const admin = await adminContext.newPage();
  await admin.goto("/");
  await admin.getByRole("link", { name: "Tenants" }).click();
  await expect(admin).toHaveURL(/\/admin\/tenants$/);
  const row = admin.getByRole("row", { name });
  await expect(async () => {
    await admin.getByLabel("Search tenants").fill(slug);
    await admin.getByRole("button", { name: "Search" }).click();
    await expect(row).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(row.locator("[data-tenant-status]")).toHaveText("Running");

  // Nobody touches it: it stays open, as nothing needs the room.
  await admin.waitForTimeout(3000);
  await admin.reload();
  await expect(row.locator("[data-tenant-status]")).toHaveText("Running");

  // Suspend: offline for the owner, closed on the node.
  await row.getByRole("link", { name }).click();
  await expect(admin.getByRole("heading", { name })).toBeVisible();
  // The invitation the owner sent is still waiting (read model, so it may lag).
  await expect(async () => {
    await admin.reload();
    await expect(admin.locator("[data-pending-invitations]")).toHaveText("1 pending invitation", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await admin.getByLabel("Reason").fill("unpaid invoices");
  await admin.getByRole("button", { name: "Suspend tenant" }).click();
  await expect(admin.getByRole("status")).toContainText("Tenant suspended.");
  await expect(admin.locator("[data-tenant-status]")).toContainText("Suspended: unpaid invoices");
  await owner.goto(`/t/${slug}`);
  await expect(owner.getByRole("alert").filter({ hasText: "suspended by the platform" })).toContainText("unpaid invoices");
  await expect(owner.locator("[data-tenant-status]")).toHaveAttribute("data-tenant-status", "stopped");

  // Resume: back online on the next visit.
  await admin.getByRole("button", { name: "Resume tenant" }).click();
  await expect(admin.getByRole("status")).toContainText("Tenant resumed.");
  await expect(admin.locator("[data-tenant-status]")).toContainText("Stopped");
  await owner.goto(`/t/${slug}`);
  await expect(owner.locator("[data-tenant-status]")).toHaveAttribute("data-tenant-status", "running");

  await ownerContext.close();
  await adminContext.close();
});
