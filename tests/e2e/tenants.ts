import { expect, type Page } from "@playwright/test";

/// A name no other test run used, and the slug the server derives from it.
export function uniqueTenant(prefix: string): { name: string; slug: string } {
  const id = `${Date.now().toString(36)}${Math.floor(Math.random() * 1000)}`;
  return { name: `${prefix} ${id}`, slug: `${prefix.toLowerCase().replace(/\s+/g, "-")}-${id}` };
}

/// Fills the creation form and lands on the new tenant's page.
export async function createTenant(page: Page, name: string) {
  await page.goto("/tenants/new");
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Create tenant" }).click();
  await expect(page).toHaveURL(/\/t\/[a-z0-9-]+\?/);
}
