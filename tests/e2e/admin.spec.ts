import { expect, test } from "@playwright/test";
import { PASSWORD, registerWithEmail, uniqueEmail } from "./email";
import { ADMIN_NAME, ADMIN_STATE } from "./fixtures";

// The administrator's pages. The admin is the first registered user, created
// by admin.setup.ts on a fresh database (`make e2e`).

test("members get no admin pages", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("member"));
  await expect(page.getByRole("link", { name: "Users" })).toHaveCount(0);
  const response = await page.goto("/admin/users");
  expect(response?.status()).toBe(403);
});

test("the administrator lists, signs out, promotes and deletes users", async ({ browser }) => {
  const memberName = uniqueEmail("carol");
  const memberContext = await browser.newContext();
  const memberPage = await memberContext.newPage();
  await registerWithEmail(memberPage, memberName);

  const adminContext = await browser.newContext({ storageState: ADMIN_STATE });
  const admin = await adminContext.newPage();
  await admin.goto("/");
  await expect(admin.locator("header").getByText("Signed in as")).toContainText(ADMIN_NAME);
  await admin.getByRole("link", { name: "Users" }).click();
  await expect(admin).toHaveURL(/\/admin\/users$/);
  await expect(admin.getByRole("cell", { name: "Administrator" }).first()).toBeVisible();

  // Search finds the member once the read model has caught up (it follows
  // the event stream with a short delay).
  const row = admin.getByRole("row", { name: memberName });
  await expect(async () => {
    await admin.getByLabel("Search users").fill(memberName);
    await admin.getByRole("button", { name: "Search" }).click();
    await expect(row).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(row).toContainText("Member");
  await expect(row).toContainText("password, 0 providers, 0 passkeys");
  await row.getByRole("link", { name: memberName }).click();
  await expect(admin.getByRole("heading", { name: memberName })).toBeVisible();
  await expect(admin.locator("section", { hasText: "Sessions" }).locator("li")).toHaveCount(1);

  // Force sign-out: the member's next request lands on the login page.
  await admin.getByRole("button", { name: "Sign out everywhere" }).click();
  await expect(admin.getByRole("status")).toContainText("Signed out everywhere.");
  await expect(async () => {
    await admin.reload();
    await expect(admin.getByText("No active session.")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await memberPage.goto("/auth/account");
  await expect(memberPage).toHaveURL(/\/auth\/login$/);

  // Roles.
  await admin.getByRole("button", { name: "Make administrator" }).click();
  await expect(admin.getByRole("status")).toContainText("granted");
  await expect(admin.getByText("Administrator", { exact: true })).toBeVisible();
  await admin.getByRole("button", { name: "Revoke administrator" }).click();
  await expect(admin.getByRole("status")).toContainText("revoked");

  // The admin cannot act on themself.
  await admin.goto("/admin/users");
  await admin.getByRole("row", { name: ADMIN_NAME }).getByRole("link", { name: ADMIN_NAME }).click();
  await expect(admin.getByText("You cannot change your own role or delete your own account.")).toBeVisible();

  // Delete the member: gone from sign-in, still listed as deleted.
  await admin.goto("/admin/users");
  await admin.getByLabel("Search users").fill(memberName);
  await admin.getByRole("button", { name: "Search" }).click();
  await admin.getByRole("link", { name: memberName }).click();
  await admin.getByRole("button", { name: "Delete account" }).click();
  await expect(admin).toHaveURL(/\/admin\/users\?/);
  await expect(admin.getByRole("status")).toContainText("Account deleted.");
  await expect(async () => {
    await admin.getByLabel("Search users").fill(memberName);
    await admin.getByRole("button", { name: "Search" }).click();
    await expect(admin.getByRole("row", { name: memberName })).toContainText("deleted", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  // Her password no longer signs in.
  await memberPage.goto("/auth/login");
  await memberPage.getByLabel("E-mail address").fill(memberName);
  await memberPage.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await memberPage.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(memberPage).toHaveURL(/\/auth\/login\?error=invalid_credentials$/);

  await memberContext.close();
  await adminContext.close();
});
