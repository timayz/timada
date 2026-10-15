import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Shipping methods: an owner adds them under /t/{slug}/shipping, changes and
// removes them from their page. A member whose role lacks `shipping:manage`
// cannot open the pages. Shoppers picking one are covered in
// `storefront.spec.ts`.

test("an owner adds, changes and removes shipping methods", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Ship Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Shipping", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/shipping$`));
  await expect(page.getByText("No shipping method: shipping is included.")).toBeVisible();

  await page.getByLabel("Name").fill("Tracked delivery");
  await page.getByLabel("Price").fill("4,9");
  await page.getByLabel("Free from (optional)").fill("50");
  await page.getByLabel("Countries (optional)").fill("fr, be");
  await page.getByRole("button", { name: "Add shipping method" }).click();
  await expect(page.getByRole("status")).toContainText("Shipping method added.");
  const tracked = page.locator('[data-shipping-method="Tracked delivery"]');
  await expect(tracked.locator("[data-shipping-price]")).toHaveText("4.90 EUR");
  await expect(tracked.locator("[data-shipping-free]")).toHaveText("From 50.00 EUR");
  await expect(tracked.locator("[data-shipping-countries]")).toHaveText("BE, FR");

  // A country that is not a code is refused.
  await page.getByLabel("Name").fill("VIP");
  await page.getByLabel("Price").fill("6.90");
  await page.getByLabel("Countries (optional)").fill("France");
  await page.getByRole("button", { name: "Add shipping method" }).click();
  await expect(page.getByRole("alert").first()).toContainText("two-letter codes");

  // Blank countries ship everywhere.
  await page.getByLabel("Name").fill("VIP");
  await page.getByLabel("Price").fill("6.90");
  await page.getByRole("button", { name: "Add shipping method" }).click();
  await expect(page.getByRole("status")).toContainText("Shipping method added.");
  const vip = page.locator('[data-shipping-method="VIP"]');
  await expect(vip.locator("[data-shipping-countries]")).toHaveText("Everywhere");
  await expect(vip.locator("[data-shipping-free]")).toHaveText("Never");

  await vip.getByRole("link", { name: "VIP" }).click();
  await expect(page.locator("[data-shipping-name]")).toHaveText("VIP");
  await expect(page.getByLabel("Price")).toHaveValue("6.90");
  await page.getByLabel("Name").fill("VIP: insured");
  await page.getByLabel("Price").fill("7.90");
  await page.getByLabel("Countries (optional)").fill("FR");
  await page.getByRole("button", { name: "Save shipping method" }).click();
  await expect(page.getByRole("status")).toContainText("Shipping method saved.");
  const insured = page.locator('[data-shipping-method="VIP: insured"]');
  await expect(insured.locator("[data-shipping-price]")).toHaveText("7.90 EUR");
  await expect(insured.locator("[data-shipping-countries]")).toHaveText("FR");

  await tracked.getByRole("link", { name: "Tracked delivery" }).click();
  await page.getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("status")).toContainText("Shipping method removed.");
  await expect(tracked).toHaveCount(0);
  await expect(insured).toBeVisible();
});

test("the shipping pages need shipping:manage", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Ship");
  await createTenant(owner, name);

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
  await expect(viewer.getByRole("link", { name: "Shipping", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/shipping`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});
