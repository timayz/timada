import { expect, test, type Page } from "@playwright/test";
import { enterCode, latestCode, mailCount, registerWithEmail, uniqueEmail, PASSWORD } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Invitations: a manager mails a link, the recipient joins with the carried
// role, signed out (through registration) or signed in; used and withdrawn
// links are refused; members change roles, are removed, or leave.

async function defineRole(page: Page, slug: string, role: string, label: string, permissions: RegExp[]) {
  await page.goto(`/t/${slug}/roles`);
  const define = page.locator("form", { has: page.getByRole("button", { name: "Define role" }) });
  await define.getByLabel("Role name").fill(role);
  await define.getByLabel("Label").fill(label);
  for (const permission of permissions) {
    await define.getByLabel(permission).check();
  }
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(page.getByRole("status")).toContainText("Role defined.");
}

test("an invited address joins through registration and gets its role", async ({ browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Invite Shop");
  await createTenant(owner, name);
  await defineRole(owner, slug, "support", "Support", [/See members and pending invitations/]);
  await defineRole(owner, slug, "manager", "Manager", [/Invite and remove members/]);

  const guestEmail = uniqueEmail("guest");
  await owner.goto(`/t/${slug}/members`);
  await invite(owner, guestEmail.toUpperCase(), "Support");
  // The pending list is a read model: it may lag a moment.
  await expect(async () => {
    await owner.reload();
    await expect(owner.locator(`[data-invitation-email="${guestEmail.toLowerCase()}"]`)).toContainText("Support", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  // The guest opens the link signed out, creates an account and comes back.
  const guestContext = await browser.newContext();
  const guest = await guestContext.newPage();
  const link = await inviteLink(guest.request, guestEmail);
  await guest.goto(link);
  await expect(guest.locator("[data-invitation=sign-in]")).toContainText(`join ${name} as Support`);
  const before = await mailCount(guest.request, guestEmail);
  await guest.getByRole("link", { name: "Create an account" }).click();
  await expect(guest).toHaveURL(/\/auth\/register\?email=/);
  await expect(guest.getByLabel("E-mail address")).toHaveValue(guestEmail.toLowerCase());
  await guest.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await guest.getByRole("button", { name: "Create account" }).click();
  await expect(guest).toHaveURL(/\/auth\/email\/verify/);
  await enterCode(guest, await latestCode(guest.request, guestEmail, before));
  await expect(guest).toHaveURL(/\/invitations\//);
  await expect(guest.locator("[data-invitation=open]")).toContainText(`join ${name} as Support`);
  await guest.getByRole("button", { name: "Accept the invitation" }).click();
  await expect(guest).toHaveURL(new RegExp(`/t/${slug}\\?`));
  await expect(guest.getByRole("status")).toContainText(`You joined ${name}.`);
  await expect(guest.locator("dd", { hasText: "Support" })).toBeVisible();

  // Support reads members, nothing more.
  await guest.getByRole("link", { name: "Members", exact: true }).click();
  await expect(guest.getByText("(you)")).toBeVisible();
  await expect(guest.getByRole("button", { name: "Send invitation" })).toHaveCount(0);
  expect((await guest.goto(`/t/${slug}/roles`))?.status()).toBe(403);
  // ...the members history included (a read model: it may lag a moment).
  await expect(async () => {
    await guest.goto(`/t/${slug}/members/history`);
    await expect(guest.locator("[data-audit-row]", { hasText: "joined as Support" })).toHaveCount(1, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  // The used link now just points the member at the tenant.
  await guest.goto(link);
  await expect(guest.locator("[data-invitation=member]")).toBeVisible();

  // The owner promotes the guest to manager: the guest can now invite, on the
  // very next request.
  await expect(async () => {
    await owner.goto(`/t/${slug}/members`);
    await expect(owner.locator("[data-invitation-email]")).toHaveCount(0, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  const guestRow = owner.getByRole("row", { name: guestEmail.toLowerCase() });
  await guestRow.getByRole("combobox").selectOption({ label: "Manager" });
  await guestRow.getByRole("button", { name: "Change role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role changed.");
  await guest.goto(`/t/${slug}/members`);
  await invite(guest, uniqueEmail("third"), "Support");
  // ...but may not hand out owner seats.
  await expect(guest.getByLabel("Invite as").locator("option", { hasText: "owner" })).toHaveCount(0);

  // Removed: the pages close.
  await owner.goto(`/t/${slug}/members`);
  await guestRow.getByRole("button", { name: "Remove" }).click();
  await expect(owner.getByRole("status")).toContainText("Member removed.");
  expect((await guest.goto(`/t/${slug}`))?.status()).toBe(403);

  // The history says who did each of it, newest first.
  await owner.getByRole("link", { name: "History" }).click();
  await expect(owner).toHaveURL(new RegExp(`/t/${slug}/members/history`));
  const rows = owner.locator("[data-audit-row]");
  const guestLabel = guestEmail.toLowerCase();
  await expect(async () => {
    await owner.reload();
    await expect(rows.first()).toContainText(`removed ${guestLabel}`, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(rows.filter({ hasText: `moved ${guestLabel} to Manager` })).toHaveCount(1);
  await expect(rows.filter({ hasText: /invited third-.* as Support/ })).toContainText(guestLabel);
  await expect(rows.filter({ hasText: `invited ${guestLabel} as Support` })).toHaveCount(1);
  await expect(rows.filter({ hasText: "joined as Support" })).toContainText(guestLabel);

  await ownerContext.close();
  await guestContext.close();
});

test("a signed-in user accepts directly; withdrawn links are refused; members leave", async ({ browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Direct");
  await createTenant(owner, name);

  const guestEmail = uniqueEmail("guest");
  const otherEmail = uniqueEmail("other");
  await owner.goto(`/t/${slug}/members`);
  await invite(owner, guestEmail, "owner");
  await invite(owner, otherEmail, "owner");
  const guestLink = await inviteLink(owner.request, guestEmail);
  const otherLink = await inviteLink(owner.request, otherEmail);

  // Withdraw the second one.
  await expect(async () => {
    await owner.reload();
    await expect(owner.locator(`[data-invitation-email="${otherEmail}"]`)).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await owner.locator(`[data-invitation-email="${otherEmail}"]`).getByRole("button", { name: "Withdraw" }).click();
  await expect(owner.getByRole("status")).toContainText("Invitation withdrawn.");

  // Already signed in: accept in one click. The invited address does not
  // have to match the account.
  const guestContext = await browser.newContext();
  const guest = await guestContext.newPage();
  await registerWithEmail(guest, uniqueEmail("someone-else"));
  await guest.goto(otherLink);
  await expect(guest.locator("[data-invitation=invalid]")).toBeVisible();
  await guest.goto(guestLink);
  await expect(guest.locator("[data-invitation=open]")).toContainText(`join ${name} as owner`);
  await guest.getByRole("button", { name: "Accept the invitation" }).click();
  await expect(guest.getByRole("status")).toContainText(`You joined ${name}.`);
  await guest.goto(guestLink);
  await expect(guest.locator("[data-invitation=member]")).toBeVisible();

  // Two owners: the first may leave now; the last one may not.
  await owner.goto(`/t/${slug}/members`);
  await owner.getByRole("button", { name: "Leave tenant" }).click();
  await expect(owner).toHaveURL(/\/\?message=/);
  await expect(owner.getByRole("status")).toContainText(`You left ${name}.`);
  expect((await owner.goto(`/t/${slug}`))?.status()).toBe(403);
  await guest.goto(`/t/${slug}/members`);
  await guest.getByRole("button", { name: "Leave tenant" }).click();
  await expect(guest).toHaveURL(/error=last_owner/);

  await ownerContext.close();
  await guestContext.close();
});
