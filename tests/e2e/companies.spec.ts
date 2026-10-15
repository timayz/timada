import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { ADMIN_STATE } from "./fixtures";
import { createTenant, fillIdentity, setLegalIdentity, uniqueTenant } from "./tenants";

// A company is who sells in law, behind one or more tenants: its SIREN is
// unique on the platform, each tenant of it numbers its invoices in a series
// of its own.

test("two shops sell as one company, each in its own series", async ({ page, browser }) => {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const shop = uniqueTenant("Main Shop");
  const pro = uniqueTenant("Pro Shop");
  await createTenant(page, shop.name);
  await createTenant(page, pro.name);
  const company = await setLegalIdentity(page, shop.slug);
  await expect(page.locator("[data-series]")).toHaveAttribute("data-series", "");

  // The second shop links to the same company: the plain series is taken,
  // so it numbers under a code of its own.
  await expect(async () => {
    await page.goto(`/t/${pro.slug}/invoicing`);
    await expect(page.getByRole("button", { name: "Link" })).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("[data-invoicing]")).toHaveAttribute("data-invoicing", "none");
  await page.getByRole("button", { name: "Link" }).click();
  await expect(page).toHaveURL(/error=series_code_taken/);
  await page.getByLabel("Series code (blank for the company's first tenant)").fill("pro");
  await page.getByRole("button", { name: "Link" }).click();
  await expect(page.getByRole("status")).toContainText("Linked to the company.");
  const year = new Date().getUTCFullYear();
  await expect(page.locator("[data-series]")).toHaveText(`PRO-F${year}-000001…`);

  // The company's page lists both, with their codes.
  await expect(async () => {
    await page.goto(company.companyUrl);
    await expect(page.locator(`[data-company-tenant="${pro.slug}"] [data-series-code]`)).toHaveAttribute(
      "data-series-code",
      "PRO",
      { timeout: 1000 },
    );
  }).toPass({ timeout: 15_000 });
  await expect(page.locator(`[data-company-tenant="${shop.slug}"] [data-series-code]`)).toHaveAttribute(
    "data-series-code",
    "",
  );

  // Someone else typing the same SIREN is turned away, until an
  // administrator releases it.
  const other = await (await browser.newContext()).newPage();
  await registerWithEmail(other, uniqueEmail("squatter"));
  await other.goto("/companies/new");
  await fillIdentity(other, company);
  await other.getByRole("button", { name: "Register" }).click();
  await expect(other).toHaveURL(/error=siren_taken/);
  await expect(other.getByRole("alert").first()).toContainText("already registered");

  const admin = await (await browser.newContext({ storageState: ADMIN_STATE })).newPage();
  await expect(async () => {
    await admin.goto("/admin/companies");
    await expect(admin.locator(`[data-admin-company="${company.siren}"]`)).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await admin.locator(`[data-admin-company="${company.siren}"]`).getByRole("button", { name: "Release the SIREN" }).click();
  await expect(admin.getByRole("status")).toContainText("SIREN released.");

  await other.goto("/companies/new");
  await fillIdentity(other, company);
  await other.getByRole("button", { name: "Register" }).click();
  await expect(other.locator("[data-company-siren]")).toHaveAttribute("data-company-siren", company.siren);
});

test("a company is its owners' only", async ({ page, browser }) => {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const shop = uniqueTenant("Owned Shop");
  await createTenant(page, shop.name);
  const company = await setLegalIdentity(page, shop.slug);

  // A stranger gets nothing at its address.
  const stranger = await (await browser.newContext()).newPage();
  const strangerEmail = uniqueEmail("partner");
  await registerWithEmail(stranger, strangerEmail);
  expect((await stranger.goto(company.companyUrl))?.status()).toBe(404);

  // Once added as an owner, the page is theirs too.
  await page.goto(company.companyUrl);
  await page.getByLabel("E-mail of the new owner").fill(strangerEmail);
  await page.getByRole("button", { name: "Add owner" }).click();
  await expect(page.getByRole("status")).toContainText("Owner added.");
  expect((await stranger.goto(company.companyUrl))?.status()).toBe(200);
  await expect(stranger.locator("[data-company-siren]")).toHaveAttribute("data-company-siren", company.siren);
});
