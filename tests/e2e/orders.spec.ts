import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Orders and open carts: an owner sees both sections under /t/{slug}; a key
// can be limited to selling (store:checkout); a member whose role lacks
// `orders:read` cannot open the pages. Orders themselves are placed by a
// storefront: see storefront.spec.ts.

test("an owner sees the orders and the open carts", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Orders Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Orders", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/orders$`));
  await expect(page.getByText("No order yet.")).toBeVisible();
  await page.getByLabel("Status").selectOption({ label: "Paid" });
  await page.getByRole("button", { name: "Filter" }).click();
  await expect(page).toHaveURL(/status=paid/);
  await expect(page.getByText("No order yet.")).toBeVisible();

  await page.getByRole("link", { name: "Carts", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/carts$`));
  await expect(page.getByText("No open cart.")).toBeVisible();

  // A storefront that sells gets a key that may do nothing else.
  await page.getByRole("link", { name: "API keys", exact: true }).click();
  await page.getByLabel("Label").fill("Shop front");
  await page.getByRole("checkbox", { name: "store:checkout" }).first().check();
  await page.getByRole("button", { name: "Create key" }).click();
  await expect(page.getByRole("status")).toContainText("Key created.");
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  const row = page.locator(`[data-api-key="${token.slice(0, 11)}"]`);
  await expect(async () => {
    await page.reload();
    await expect(row.locator("[data-api-key-scopes]")).toHaveAttribute(
      "data-api-key-scopes",
      "store:checkout store:read",
      { timeout: 1000 },
    );
  }).toPass({ timeout: 15_000 });
});

test("the orders pages need orders:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Orders");
  await createTenant(owner, name);

  // A "viewer" role with no permission at all.
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
  await expect(viewer.getByRole("link", { name: "Orders", exact: true })).toHaveCount(0);
  await expect(viewer.getByRole("link", { name: "Carts", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/orders`))?.status()).toBe(403);
  expect((await viewer.goto(`/t/${slug}/carts`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});
