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

/// A SIREN no other test run used, with a SIRET and a French VAT number that
/// pass their checks: SIRENs are unique on the platform.
export function uniqueCompanyNumbers(): { siren: string; siret: string; vat: string } {
  const luhn = (digits: string) =>
    digits
      .split("")
      .reverse()
      .map(Number)
      .reduce((sum, d, i) => {
        if (i % 2 === 1) {
          const twice = d * 2;
          return sum + (twice > 9 ? twice - 9 : twice);
        }
        return sum + d;
      }, 0) % 10;
  const withCheckDigit = (body: string) => {
    for (let d = 0; d <= 9; d++) {
      if (luhn(body + d) === 0) return body + d;
    }
    throw new Error("no check digit");
  };
  const base = `${Date.now()}${Math.floor(Math.random() * 1000)}`.slice(-8);
  const siren = withCheckDigit(base.replace(/^3560/, "1560"));
  const siret = withCheckDigit(`${siren}0001`);
  const key = (12 + 3 * (Number(siren) % 97)) % 97;
  return { siren, siret, vat: `FR${String(key).padStart(2, "0")}${siren}` };
}

/// Fills the identity form of a company (on its page, or registering one).
export async function fillIdentity(page: Page, numbers: { siret: string; vat: string }, vatRegistered = true) {
  await page.getByLabel("Legal name").fill("Acme Boutique SAS");
  await page.getByLabel("Legal form (SAS, SARL, EI…)").fill("SAS");
  await page.getByLabel("Share capital in euros (blank for none)").fill("10000");
  await page.getByLabel("SIRET").fill(numbers.siret);
  await page.getByLabel("RCS city (blank if not registered)").fill("Paris");
  await page.getByLabel("Contact e-mail").fill("hello@acme.example");
  await page.getByLabel("Address", { exact: true }).fill("1 rue de la Paix");
  await page.getByLabel("Postal code").fill("75002");
  await page.getByLabel("City", { exact: true }).fill("Paris");
  await page
    .getByLabel("Intra-community VAT number (blank under the exemption)")
    .fill(vatRegistered ? numbers.vat : "");
}

/// Makes the tenant sell as a newly registered company, which a sale
/// needs: a VAT-registered SAS, not registered for the EU One-Stop Shop.
/// Returns the company's numbers and the address of its page.
export async function setLegalIdentity(page: Page, slug: string) {
  const numbers = uniqueCompanyNumbers();
  await page.goto(`/t/${slug}/invoicing`);
  await page.getByRole("link", { name: "Register a new company" }).click();
  await fillIdentity(page, numbers);
  await page.getByRole("button", { name: "Register" }).click();
  await expect(page.getByRole("status")).toContainText("Company registered and linked.");
  const companyUrl = (await page.locator("[data-linked-company] a").getAttribute("href")) ?? "";
  expect(companyUrl).toMatch(/\/companies\/[0-9A-Z]{26}/);
  return { ...numbers, companyUrl };
}
