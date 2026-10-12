import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// Custom roles: an owner defines, edits and removes them under
// /t/{slug}/roles; the members page lists the seats; non-members get 403.

test("an owner defines, edits and removes roles", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Roles Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Roles", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/roles$`));
  await expect(page.locator("[data-role=owner]")).toContainText("1 member(s)");
  // Each role has its own edit form, so scope the "define" one.
  const define = page.locator("form", { has: page.getByRole("button", { name: "Define role" }) });

  // Define "support" with one permission.
  await define.getByLabel("Role name").fill("Support");
  await define.getByLabel("Label").fill("Support team");
  await define.getByLabel(/See members and pending invitations/).check();
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(page.getByRole("status")).toContainText("Role defined.");
  const support = page.locator("[data-role=support]");
  await expect(support.getByRole("heading")).toContainText("Support team");
  await expect(support.getByLabel(/See members and pending invitations/)).toBeChecked();
  await expect(support.getByLabel(/Define, edit and remove roles/)).not.toBeChecked();

  // The same name again is refused; so is the reserved one.
  await define.getByLabel("Role name").fill("support");
  await define.getByLabel("Label").fill("Again");
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(page).toHaveURL(/error=role_exists/);
  await expect(page.getByRole("alert").first()).toContainText("already exists");
  await define.getByLabel("Role name").fill("owner");
  await define.getByLabel("Label").fill("Owner");
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(page).toHaveURL(/error=reserved_role/);

  // Edit: new label, one more permission.
  await support.getByLabel("Label").fill("Support desk");
  await support.getByLabel(/Invite, change roles and remove members/).check();
  await support.getByRole("button", { name: "Save role" }).click();
  await expect(page.getByRole("status")).toContainText("Role saved.");
  await expect(support.getByRole("heading")).toContainText("Support desk");
  await expect(support.getByLabel(/Invite, change roles and remove members/)).toBeChecked();

  // The members page shows the owner alone, who cannot leave.
  await page.getByRole("link", { name: "Members", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/members$`));
  await expect(page.getByRole("row")).toHaveCount(2);
  await expect(page.getByText("(you)")).toBeVisible();
  await page.getByRole("button", { name: "Leave tenant" }).click();
  await expect(page).toHaveURL(/error=last_owner/);
  await expect(page.getByRole("alert").first()).toContainText("at least one owner");

  // Remove the role.
  await page.getByRole("link", { name: "Roles", exact: true }).click();
  await support.getByRole("button", { name: "Remove role" }).click();
  await expect(page.getByRole("status")).toContainText("Role removed.");
  await expect(page.locator("[data-role=support]")).toHaveCount(0);
});

test("roles and members are closed to non-members", async ({ browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Closed");
  await createTenant(owner, name);

  const strangerContext = await browser.newContext();
  const stranger = await strangerContext.newPage();
  await registerWithEmail(stranger, uniqueEmail("stranger"));
  expect((await stranger.goto(`/t/${slug}/roles`))?.status()).toBe(403);
  expect((await stranger.goto(`/t/${slug}/members`))?.status()).toBe(403);

  await ownerContext.close();
  await strangerContext.close();
});
