import { expect, test, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

/// Opens the discount list and waits for `code` to be in it: the list comes
/// from a read model, a moment behind the write.
async function openDiscounts(page: Page, tenant: string, code: string) {
  await expect(async () => {
    await page.goto(`/t/${tenant}/discounts`);
    await expect(page.locator(`[data-discount=${code}]`)).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
}

// Discount codes: an owner creates them under /t/{slug}/discounts,
// deactivates and archives them from their page. A member whose role lacks
// `discounts:read` cannot open the pages. Shoppers using them are covered in
// `storefront.spec.ts`.

test("an owner creates, deactivates and archives a discount code", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Promo Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Discounts", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/discounts$`));
  await expect(page.getByText("No discount yet.")).toBeVisible();

  // A percentage, from a day on, capped, once per e-mail.
  await page.getByLabel("Code").fill("summer10");
  await page.getByLabel("Percentage or amount").fill("12.5");
  await page.getByLabel("First day (optional)").fill("2026-06-01");
  await page.getByLabel("Last day (optional)").fill("2026-08-31");
  await page.getByLabel("Orders it serves at most (optional)").fill("100");
  await page.getByLabel("Once per shopper e-mail").check();
  await page.getByRole("button", { name: "Create discount" }).click();
  await expect(page.getByRole("status")).toContainText("Discount created.");
  await expect(page.locator("[data-discount-code]")).toHaveText("SUMMER10");
  await expect(page.locator("[data-discount-value]")).toContainText("12.5 %");
  await expect(page.getByText("2026-06-01 to 2026-08-31")).toBeVisible();
  await expect(page.getByText("100 orders, once per e-mail")).toBeVisible();
  await expect(page.locator("[data-discount-used]")).toContainText("0 of 100");

  // The code is unique in the tenant, whatever its case.
  await openDiscounts(page, slug, "SUMMER10");
  await page.getByLabel("Code").fill("Summer10");
  await page.getByLabel("Percentage or amount").fill("5");
  await page.getByRole("button", { name: "Create discount" }).click();
  await expect(page.getByRole("alert").first()).toContainText("already uses this code");

  // An amount off.
  await page.getByLabel("Code").fill("FIVE-OFF");
  await page.getByRole("radio", { name: "an amount" }).check();
  await page.getByLabel("Percentage or amount").fill("5");
  await page.getByLabel("Currency of an amount").selectOption("EUR");
  await page.getByRole("button", { name: "Create discount" }).click();
  await expect(page.getByRole("status")).toContainText("Discount created.");
  await expect(page.locator("[data-discount-value]")).toContainText("5.00");

  await page.getByRole("button", { name: "Deactivate" }).click();
  await expect(page.getByRole("status")).toContainText("Discount deactivated.");
  await expect(page.locator("[data-discount-status]")).toHaveAttribute("data-discount-status", "inactive");
  await page.getByRole("button", { name: "Reactivate" }).click();
  await expect(page.getByRole("status")).toContainText("Discount reactivated.");
  await page.getByRole("button", { name: "Archive" }).click();
  await expect(page.getByRole("status")).toContainText("Discount archived.");
  await expect(page.getByRole("button", { name: "Archive" })).toHaveCount(0);

  // Archived, it leaves the list and its code is free again.
  await expect(async () => {
    await page.goto(`/t/${slug}/discounts`);
    await expect(page.locator("[data-discount=FIVE-OFF]")).toHaveCount(0, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.getByLabel("Code").fill("five-off");
  await page.getByLabel("Percentage or amount").fill("20");
  await page.getByRole("button", { name: "Create discount" }).click();
  await expect(page.getByRole("status")).toContainText("Discount created.");
});

test("the discount pages need discounts:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Promo");
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
  await expect(viewer.getByRole("link", { name: "Discounts", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/discounts`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});
