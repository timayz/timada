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

/// Says who the merchant is in law, which a sale needs: a VAT-registered
/// SAS, not registered for the EU One-Stop Shop.
export async function setLegalIdentity(page: Page, slug: string, vatNumber = "FR85120027016") {
  await page.goto(`/t/${slug}/invoicing`);
  await page.getByLabel("Legal name").fill("Acme Boutique SAS");
  await page.getByLabel("Legal form (SAS, SARL, EI…)").fill("SAS");
  await page.getByLabel("Share capital in euros (blank for none)").fill("10000");
  await page.getByLabel("SIRET").fill("120 027 016 00092");
  await page.getByLabel("RCS city (blank if not registered)").fill("Paris");
  await page.getByLabel("Contact e-mail").fill("hello@acme.example");
  await page.getByLabel("Address", { exact: true }).fill("1 rue de la Paix");
  await page.getByLabel("Postal code").fill("75002");
  await page.getByLabel("City", { exact: true }).fill("Paris");
  await page.getByLabel("Intra-community VAT number (blank under the exemption)").fill(vatNumber);
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Legal identity saved.");
}
