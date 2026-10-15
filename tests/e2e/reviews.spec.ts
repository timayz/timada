import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Reviews: shoppers' reviews are listed under /t/{slug}/reviews for members
// with `reviews:read`. Writing and moderating one is covered in
// `storefront.spec.ts`, where a shopper first pays for an order.

test("an owner sees the reviews section, empty at first", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Quiet Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Reviews", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/reviews$`));
  await expect(page.getByLabel("Status")).toHaveValue("pending");
  await expect(page.getByText("No review here.")).toBeVisible();
});

test("the review pages need reviews:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Reviews");
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
  await expect(viewer.getByRole("link", { name: "Reviews", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/reviews`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});
