import { expect, test, type APIRequestContext, type Browser, type Page } from "@playwright/test";
import { MAILDEV, PASSWORD, signInWithEmail, uniqueEmail } from "./email";
import { ADMIN_STATE } from "./fixtures";
import { invite, inviteLink } from "./invitations";
import { expectSignedInAs, signOut } from "./shell";
import { createTenant, uniqueTenant } from "./tenants";

// Root's invitations to create an account: root has a link mailed to an
// address from the page of users, and the link creates the account of that
// address, once. They work whether sign-up is open or closed.
//
// The last tests are about a server whose sign-up is closed
// (`auth.signup = "invitation"`), and are skipped on the suite's servers,
// which leave it open. To run them, alone, on such a server:
//
//   TIMADA__AUTH__SIGNUP=invitation E2E_SHARDS=1 scripts/e2e.sh \
//     tests/e2e/signup-invitations.spec.ts

type MailDevMessage = { subject: string; html?: string; text?: string; date: string };

/// The link in the newest invitation to create an account mailed to `email`.
async function accountInviteLink(request: APIRequestContext, email: string): Promise<string> {
  let link = "";
  await expect(async () => {
    const response = await request.get(`${MAILDEV}/email`, { params: { "to.address": email } });
    expect(response.ok()).toBe(true);
    const messages = ((await response.json()) as MailDevMessage[])
      .filter((m) => m.subject === "You are invited to create your Timada account")
      .sort((a, b) => new Date(a.date).getTime() - new Date(b.date).getTime());
    expect(messages.length, `no invitation mailed to ${email} yet`).toBeGreaterThan(0);
    const last = messages[messages.length - 1];
    const match = (last.html ?? last.text ?? "").match(/https?:\/\/[^\s"<]+\/auth\/invitation\/[A-Za-z0-9_-]+/);
    expect(match, "no link in the invitation").not.toBeNull();
    link = match![0];
  }).toPass({ timeout: 15_000 });
  return link;
}

/// Root's page of users, signed in.
async function rootPage(browser: Browser): Promise<Page> {
  const context = await browser.newContext({ storageState: ADMIN_STATE });
  const page = await context.newPage();
  await page.goto("/admin/users");
  return page;
}

/// From root's page of users: invites `email`.
async function inviteAccount(root: Page, email: string) {
  const form = root.locator("form", { has: root.getByRole("button", { name: "Send invitation" }) });
  await form.getByLabel("E-mail address").fill(email);
  await form.getByRole("button", { name: "Send invitation" }).click();
  await expect(root.getByRole("status")).toContainText(`Sending the invitation to ${email.toLowerCase()}`);
}

/// Opens the invitation's link and creates the account with `PASSWORD`.
async function createFromLink(page: Page, link: string, email: string) {
  await page.goto(link);
  await expect(page.locator("[data-account-invitation=open]")).toBeVisible();
  const address = page.getByLabel("E-mail address");
  await expect(address).toHaveValue(email);
  await expect(address).not.toBeEditable();
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
}

/// An account for `email`, the way that works whatever `auth.signup` says:
/// root invites it. Returns its page, signed in.
async function invitedAccount(browser: Browser, root: Page, email: string): Promise<Page> {
  await root.goto("/admin/users");
  await inviteAccount(root, email);
  const context = await browser.newContext();
  const page = await context.newPage();
  await createFromLink(page, await accountInviteLink(page.request, email), email);
  await expect(page).toHaveURL(/\/$/);
  return page;
}

/// Whether this server's sign-up is closed: its login page says so.
async function signupClosed(page: Page): Promise<boolean> {
  await page.goto("/auth/login");
  return (await page.locator("[data-signup=invitation]").count()) > 0;
}

test("root invites an address, whose link creates its account once", async ({ browser }) => {
  const root = await rootPage(browser);
  const email = uniqueEmail("invited");
  await inviteAccount(root, email.toUpperCase());

  // The pending list is a read model: it may lag a moment, and says the
  // link was mailed once the mailroom sent it.
  const pending = root.locator(`[data-invitation-email="${email}"]`);
  await expect(async () => {
    await root.reload();
    await expect(pending.locator("[data-invitation-mail=sent]")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  const guestContext = await browser.newContext();
  const guest = await guestContext.newPage();
  const link = await accountInviteLink(guest.request, email);
  // No code is mailed: the link proved the address.
  await createFromLink(guest, link, email);
  await expect(guest).toHaveURL(/\/$/);
  await expectSignedInAs(guest, email);

  // The link worked once.
  const again = await (await browser.newContext()).newPage();
  await again.goto(link);
  await expect(again.locator("[data-account-invitation=invalid]")).toContainText("no longer valid");
  await expect(again.getByRole("button", { name: "Create account" })).toHaveCount(0);

  // Used, it left root's list; the account is on the list of users.
  await expect(async () => {
    await root.reload();
    await expect(pending).toHaveCount(0, { timeout: 1000 });
    await root.getByLabel("Search users").fill(email);
    await root.getByRole("button", { name: "Search" }).click();
    await expect(root.getByRole("row", { name: email })).toContainText("password", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });

  // The password chosen on the invitation signs in, with a mailed code as
  // for any account.
  await signOut(guest);
  await signInWithEmail(guest, email);
  await expectSignedInAs(guest, email);
});

test("a withdrawn invitation creates nothing, and an account is not invited again", async ({ browser }) => {
  const root = await rootPage(browser);
  const email = uniqueEmail("withdrawn");
  await inviteAccount(root, email);
  const guest = await (await browser.newContext()).newPage();
  const link = await accountInviteLink(guest.request, email);
  await guest.goto(link);
  await expect(guest.locator("[data-account-invitation=open]")).toBeVisible();

  const pending = root.locator(`[data-invitation-email="${email}"]`);
  await expect(async () => {
    await root.reload();
    await expect(pending).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await pending.getByRole("button", { name: "Withdraw" }).click();
  await expect(root.getByRole("status")).toContainText("Invitation withdrawn.");

  // The page was open before the withdrawal: its form creates nothing.
  await guest.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await guest.getByRole("button", { name: "Create account" }).click();
  await expect(guest).toHaveURL(/\/auth\/invitation\/.*error=invitation_invalid/);
  await expect(guest.locator("[data-account-invitation=invalid]")).toBeVisible();

  // An address with an account is refused.
  const taken = uniqueEmail("taken");
  const member = await invitedAccount(browser, root, taken);
  await root.goto("/admin/users");
  const form = root.locator("form", { has: root.getByRole("button", { name: "Send invitation" }) });
  await form.getByLabel("E-mail address").fill(taken);
  await form.getByRole("button", { name: "Send invitation" }).click();
  await expect(root.getByRole("alert").first()).toContainText("already registered");

  // Inviting is root's alone: a member has no page of users.
  expect((await member.goto("/admin/users"))?.status()).toBe(403);
});

test("an administrator who is not root sees no invitations", async ({ browser }) => {
  const root = await rootPage(browser);
  const email = uniqueEmail("admin");
  const admin = await invitedAccount(browser, root, email);

  await expect(async () => {
    await root.goto("/admin/users");
    await root.getByLabel("Search users").fill(email);
    await root.getByRole("button", { name: "Search" }).click();
    await expect(root.getByRole("row", { name: email })).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await root.getByRole("link", { name: email }).click();
  await root.getByRole("button", { name: "Make administrator" }).click();
  await expect(root.getByRole("status")).toContainText("granted");

  await admin.goto("/admin/users");
  await expect(admin.getByRole("heading", { name: "Users" })).toBeVisible();
  await expect(admin.getByRole("button", { name: "Send invitation" })).toHaveCount(0);
  await expect(admin.getByText("Pending invitations")).toHaveCount(0);
});

test.describe("with sign-up closed", () => {
  test.beforeEach(async ({ page }) => {
    test.skip(!(await signupClosed(page)), "sign-up is open on this server");
  });

  test("nobody creates an account on their own", async ({ page }) => {
    await page.goto("/auth/login");
    await expect(page.getByRole("link", { name: "Create an account" })).toHaveCount(0);
    await expect(page.locator("[data-signup=invitation]")).toContainText("Accounts are created by invitation.");

    await page.goto("/auth/register");
    await expect(page).toHaveURL(/\/auth\/login\?error=signup_closed$/);
    await expect(page.getByRole("alert").first()).toContainText("created by invitation");

    // A provider signs in the accounts that exist, and creates none.
    const provider = page.getByRole("button", { name: "Continue with Dex" });
    if ((await provider.count()) > 0) {
      await provider.click();
      await page.waitForURL(/localhost:5556\/dex\//);
      await page.locator("#login").fill("admin@example.com");
      await page.locator("#password").fill("password");
      await page.locator("#submit-login").click();
      await expect(page).toHaveURL(/\/auth\/login\?error=not_registered$/);
    }
  });

  test("a provider creates the invited account for the address it verified, and no other", async ({ browser }) => {
    const root = await rootPage(browser);
    const probe = await (await browser.newContext()).newPage();
    await probe.goto("/auth/login");
    test.skip((await probe.getByRole("button", { name: "Continue with Dex" }).count()) === 0, "no dex provider");

    const atDex = async (page: Page) => {
      await page.getByRole("button", { name: "Continue with Dex" }).click();
      await page.waitForURL(/localhost:5556\/dex\//);
      await page.locator("#login").fill("admin@example.com");
      await page.locator("#password").fill("password");
      await page.locator("#submit-login").click();
    };

    // Another address than the one dex vouches for: refused, the link kept.
    const other = uniqueEmail("not-dex");
    await inviteAccount(root, other);
    const stranger = await (await browser.newContext()).newPage();
    const otherLink = await accountInviteLink(stranger.request, other);
    await stranger.goto(otherLink);
    await atDex(stranger);
    await expect(stranger).toHaveURL(/\/auth\/invitation\/.*error=invitation_other_address/);
    await expect(stranger.getByRole("alert").first()).toContainText("another e-mail address");
    await expect(stranger.locator("[data-account-invitation=open]")).toBeVisible();

    // Its own address: the account is created and signed in.
    await root.goto("/admin/users");
    await inviteAccount(root, "admin@example.com");
    const guest = await (await browser.newContext()).newPage();
    await guest.goto(await accountInviteLink(guest.request, "admin@example.com"));
    await atDex(guest);
    await expect(guest).toHaveURL(/\/(\?.*)?$/);
    await expectSignedInAs(guest, "Dex User");
  });

  test("an invitation to a shop lets an account in and creates none", async ({ browser }) => {
    const root = await rootPage(browser);
    const owner = await invitedAccount(browser, root, uniqueEmail("owner"));
    const { name, slug } = uniqueTenant("Closed Shop");
    await createTenant(owner, name);

    const guestEmail = uniqueEmail("guest");
    await owner.goto(`/t/${slug}/members`);
    await invite(owner, guestEmail, "owner");
    const guest = await (await browser.newContext()).newPage();
    const link = await inviteLink(guest.request, guestEmail);
    await guest.goto(link);
    await expect(guest.locator("[data-invitation=sign-in]")).toContainText(`join ${name}`);
    await expect(guest.getByRole("link", { name: "Create an account" })).toHaveCount(0);
    await expect(guest.locator("[data-signup=invitation]")).toContainText("created by invitation");

    // Root's invitation gives the account; the shop's link then lets it in.
    await root.goto("/admin/users");
    await inviteAccount(root, guestEmail);
    await createFromLink(guest, await accountInviteLink(guest.request, guestEmail), guestEmail);
    await expect(guest).toHaveURL(/\/invitations\//);
    await guest.getByRole("button", { name: "Accept the invitation" }).click();
    await expect(guest.getByRole("status")).toContainText(`You joined ${name}.`);
  });
});
