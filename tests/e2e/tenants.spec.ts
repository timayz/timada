import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { ADMIN_STATE } from "./fixtures";

// Tenants: self-service creation, the tenant's own page, and the platform
// administration (status, suspend, resume). The e2e configuration closes an
// idle tenant database after 2 seconds, so a test can watch it stop.

function uniqueSlug(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}${Math.floor(Math.random() * 1000)}`;
}

test("a user creates a tenant, owns it and renames it", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  await expect(page.getByText("You do not belong to any tenant yet.")).toBeVisible();

  const slug = uniqueSlug("acme");
  await page.getByRole("link", { name: "Create a tenant" }).click();
  await expect(page).toHaveURL(/\/tenants\/new$/);
  await page.getByLabel("Name").fill("Acme Shop");
  await page.getByLabel("Slug").fill(slug.toUpperCase());
  await page.getByRole("button", { name: "Create tenant" }).click();

  await expect(page).toHaveURL(new RegExp(`/t/${slug}\\?`));
  await expect(page.getByRole("status")).toContainText("Tenant created.");
  await expect(page.getByRole("heading", { name: "Acme Shop" })).toBeVisible();
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

  // The slug is taken now, whatever its case.
  await page.goto("/tenants/new");
  await page.getByLabel("Name").fill("Copycat");
  await page.getByLabel("Slug").fill(slug);
  await page.getByRole("button", { name: "Create tenant" }).click();
  await expect(page).toHaveURL(/\/tenants\/new\?error=slug_taken/);
  await expect(page.getByRole("alert").first()).toContainText("already taken");
  await expect(page.getByLabel("Name")).toHaveValue("Copycat");
});

test("only members see a tenant", async ({ browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const slug = uniqueSlug("private");
  await owner.goto("/tenants/new");
  await owner.getByLabel("Name").fill("Private");
  await owner.getByLabel("Slug").fill(slug);
  await owner.getByRole("button", { name: "Create tenant" }).click();
  await expect(owner).toHaveURL(new RegExp(`/t/${slug}`));

  const strangerContext = await browser.newContext();
  const stranger = await strangerContext.newPage();
  await registerWithEmail(stranger, uniqueEmail("stranger"));
  expect((await stranger.goto(`/t/${slug}`))?.status()).toBe(403);
  expect((await stranger.goto(`/t/${uniqueSlug("nobody")}`))?.status()).toBe(404);
  expect((await stranger.goto("/admin/tenants"))?.status()).toBe(403);

  await ownerContext.close();
  await strangerContext.close();
});

test("the administrator sees tenants stop when idle, suspends and resumes them", async ({ browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const slug = uniqueSlug("idle");
  await owner.goto("/tenants/new");
  await owner.getByLabel("Name").fill("Idle Shop");
  await owner.getByLabel("Slug").fill(slug);
  await owner.getByRole("button", { name: "Create tenant" }).click();
  await expect(owner).toHaveURL(new RegExp(`/t/${slug}`));

  const adminContext = await browser.newContext({ storageState: ADMIN_STATE });
  const admin = await adminContext.newPage();
  await admin.goto("/");
  await admin.getByRole("link", { name: "Tenants" }).click();
  await expect(admin).toHaveURL(/\/admin\/tenants$/);
  const row = admin.getByRole("row", { name: "Idle Shop" });
  await expect(async () => {
    await admin.getByLabel("Search tenants").fill(slug);
    await admin.getByRole("button", { name: "Search" }).click();
    await expect(row).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(row.locator("[data-tenant-status]")).toHaveText("Running");

  // Nobody touches it: the database closes after the idle timeout.
  await expect(async () => {
    await admin.reload();
    await expect(row.locator("[data-tenant-status]")).toHaveText("Stopped", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  // A member's visit opens it again.
  await owner.goto(`/t/${slug}`);
  await expect(owner.locator("[data-tenant-status]")).toHaveAttribute("data-tenant-status", "running");
  await admin.reload();
  await expect(row.locator("[data-tenant-status]")).toHaveText("Running");

  // Suspend: offline for the owner, closed on the node.
  await row.getByRole("link", { name: "Idle Shop" }).click();
  await expect(admin.getByRole("heading", { name: "Idle Shop" })).toBeVisible();
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
