import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Customers: the shoppers who signed in to a shop are listed under
// /t/{slug}/customers for members with `customers:read`. A shopper signing in
// and showing up there is covered in `storefront.spec.ts`.

test("an owner sees the customers section, empty at first", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("New Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Customers", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/customers$`));
  await expect(page.locator("[data-customer-count]")).toHaveText("0 customer(s)");
  await expect(page.getByText("No customer here.")).toBeVisible();
});

test("the customers page needs customers:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Customers");
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
  await expect(viewer.getByRole("link", { name: "Customers", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/customers`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});
