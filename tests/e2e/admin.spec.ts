import { expect, test, type Page } from "@playwright/test";
import { PASSWORD, registerWithEmail, uniqueEmail } from "./email";
import { ADMIN_NAME, ADMIN_STATE } from "./fixtures";
import { expectSignedInAs } from "./shell";

// The administrator's pages. The admin is the root account of
// config/e2e.toml, which admin.setup.ts signs in as.

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
  await expectSignedInAs(admin, ADMIN_NAME);
  await admin.getByRole("link", { name: "Users" }).click();
  await expect(admin).toHaveURL(/\/admin\/users$/);
  await expect(admin.getByRole("row", { name: ADMIN_NAME }).getByRole("cell", { name: "Root", exact: true })).toBeVisible();

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
  // The session is committed after the user, so the read model can list the
  // member before its session: reload until the row is there.
  const sessions = admin.locator("section", { hasText: "Sessions" }).locator("li");
  await expect(async () => {
    await admin.reload();
    await expect(sessions).toHaveCount(1, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });

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

// The balancer's question: unauthenticated, JSON, 200 while the node serves
// and its read models keep up with the store.
test("the node says it is ready, and how far its read models trail", async ({ request }) => {
  const response = await request.get("/ready");
  expect(response.status()).toBe(200);
  expect(response.headers()["cache-control"]).toBe("no-store");
  const body = await response.json();
  expect(body.ready).toBe(true);
  expect(body.serving).toBe(true);
  expect(body.node).toBe("serving");
  expect(typeof body.lag_seconds).toBe("number");
  const names = body.read_models.map((lag: { name: string }) => lag.name);
  expect(names).toContain("catalog");
  expect(names).toContain("orders");
});

test("only root makes administrators, and nobody acts on root", async ({ browser }) => {
  const aliceName = uniqueEmail("alice");
  const aliceContext = await browser.newContext();
  const alice = await aliceContext.newPage();
  await registerWithEmail(alice, aliceName);
  const bobName = uniqueEmail("bob");
  const bobContext = await browser.newContext();
  await registerWithEmail(await bobContext.newPage(), bobName);

  /// Opens a user's page from the list, once the read model lists them.
  const openUser = async (page: Page, name: string) => {
    await page.goto("/admin/users");
    await expect(async () => {
      await page.getByLabel("Search users").fill(name);
      await page.getByRole("button", { name: "Search" }).click();
      await expect(page.getByRole("row", { name })).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await page.getByRole("row", { name }).getByRole("link", { name }).click();
    await expect(page.getByRole("heading", { name })).toBeVisible();
  };

  // Root makes Alice an administrator.
  const rootContext = await browser.newContext({ storageState: ADMIN_STATE });
  const root = await rootContext.newPage();
  await openUser(root, aliceName);
  await root.getByRole("button", { name: "Make administrator" }).click();
  await expect(root.getByRole("status")).toContainText("granted");

  // Alice administers: she deletes members, and makes no administrator.
  await alice.goto("/");
  await expect(alice.getByRole("link", { name: "Users" })).toBeVisible();
  await openUser(alice, bobName);
  await expect(alice.getByText("Only the root account makes an administrator.")).toBeVisible();
  await expect(alice.getByRole("button", { name: "Make administrator" })).toHaveCount(0);
  await expect(alice.getByRole("button", { name: "Delete account" })).toBeVisible();

  // Root is out of her reach.
  await openUser(alice, ADMIN_NAME);
  await expect(alice.locator("dd").filter({ hasText: /^Root$/ })).toBeVisible();
  await expect(alice.getByText("nobody changes its role, signs it out or deletes it")).toBeVisible();
  await expect(alice.getByRole("button", { name: "Sign out everywhere" })).toHaveCount(0);
  await expect(alice.getByRole("button", { name: "Revoke administrator" })).toHaveCount(0);
  await expect(alice.getByRole("button", { name: "Delete account" })).toHaveCount(0);

  // Root's own account is the configuration's: nothing to change, nothing to add.
  await root.goto("/auth/account");
  await expect(root.getByText("This is the root account")).toBeVisible();
  await expect(root.locator("dd").filter({ hasText: /^Root$/ })).toBeVisible();
  await expect(root.getByRole("button", { name: "Change password" })).toHaveCount(0);
  await expect(root.getByRole("button", { name: "Add a passkey" })).toHaveCount(0);

  // Nobody registers with root's address.
  const visitor = await (await browser.newContext()).newPage();
  await visitor.goto("/auth/register");
  await visitor.getByLabel("E-mail address").fill(ADMIN_NAME);
  await visitor.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await visitor.getByRole("button", { name: "Create account" }).click();
  await expect(visitor).toHaveURL(/\/auth\/register\?.*error=already_registered/);

  // Root unmakes her.
  await openUser(root, aliceName);
  await root.getByRole("button", { name: "Revoke administrator" }).click();
  await expect(root.getByRole("status")).toContainText("revoked");
  expect((await alice.goto("/admin/users"))?.status()).toBe(403);

  await aliceContext.close();
  await bobContext.close();
  await rootContext.close();
  await visitor.context().close();
});
