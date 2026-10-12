import { expect, test, type Page } from "@playwright/test";
import { PASSWORD, enterCode, latestCode, mailCount, registerWithEmail, signInWithEmail, uniqueEmail } from "./email";

// The OpenID Connect flow against the dex container from compose.yml
// (config/e2e.toml registers it as the "Dex" provider). Skipped when the
// running server has no such provider.

const DEX_USER = "Dex User";

async function signInAtDex(page: Page) {
  await page.waitForURL(/localhost:5556\/dex\//);
  await page.locator("#login").fill("admin@example.com");
  await page.locator("#password").fill("password");
  await page.locator("#submit-login").click();
  await expect(page).toHaveURL(/timada\.localhost\/(\?.*)?$/);
}

test.beforeEach(async ({ page }) => {
  await page.goto("/auth/login");
  const provider = page.getByRole("button", { name: "Continue with Dex" });
  test.skip((await provider.count()) === 0, "no dex provider configured on this server");
});

test("register and sign in through the provider", async ({ page }) => {
  await page.getByRole("button", { name: "Continue with Dex" }).click();
  await signInAtDex(page);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(DEX_USER);

  await page.goto("/auth/account");
  await expect(page.locator("section", { hasText: "Sign-in providers" })).toContainText("Dex");
  await expect(page.locator("section", { hasText: "Sign-in providers" })).toContainText("admin@example.com");
  const signIns = page.locator("section", { hasText: "Recent sign-ins" }).locator("li");
  const before = await signIns.count();

  // Again: the identity is known now, so this is a sign-in, not a registration.
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page).toHaveURL(/\/auth\/login$/);
  await page.getByRole("button", { name: "Continue with Dex" }).click();
  await signInAtDex(page);
  await page.goto("/auth/account");
  await expect(signIns).toHaveCount(before + 1);
  await expect(page.getByText("No provider linked.")).toHaveCount(0);
});

test("a tampered callback is refused and the attempt cannot be replayed", async ({ page }) => {
  await page.getByRole("button", { name: "Continue with Dex" }).click();
  await page.waitForURL(/localhost:5556\/dex\//);
  // Come back with a code for a different state than the one sealed in the cookie.
  await page.goto("/auth/oauth2/dex/callback?code=forged&state=forged");
  await expect(page).toHaveURL(/\/auth\/login\?error=expired$/);
  await expect(page.getByRole("alert").first()).toContainText("expired");
  // The flow cookie is gone: a second attempt with the real state fails the same way.
  await page.goto("/auth/oauth2/dex/callback?code=forged&state=forged");
  await expect(page).toHaveURL(/\/auth\/login\?error=expired$/);
});

test("signing in through the provider while signed in switches accounts", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("local"));

  // The login page sends signed-in browsers home, so submit its form directly.
  await page.evaluate(() => {
    const form = document.createElement("form");
    form.method = "post";
    form.action = "/auth/oauth2/dex";
    const mode = document.createElement("input");
    mode.name = "mode";
    mode.value = "create_user_or_login";
    form.append(mode);
    document.body.append(form);
    form.submit();
  });
  await signInAtDex(page);
  await expect(page.getByRole("status")).toContainText(`You are now signed in as ${DEX_USER}.`);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(DEX_USER);
});

test("a provider user adds an e-mail address and a password, once", async ({ page }) => {
  await page.getByRole("button", { name: "Continue with Dex" }).click();
  await signInAtDex(page);
  await page.goto("/auth/account");
  const section = page.locator("section", { hasText: "E-mail and password" });
  await expect(section).toContainText("Not set up");

  // An address that belongs to someone else is refused before any mail.
  const taken = uniqueEmail("taken");
  const other = await page.context().browser()!.newContext();
  await registerWithEmail(await other.newPage(), taken);
  await other.close();
  await section.getByLabel("E-mail address").fill(taken);
  await section.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await section.getByRole("button", { name: "Add e-mail address" }).click();
  await expect(page).toHaveURL(/\/auth\/account\?error=already_registered$/);
  await expect(page.getByRole("alert").first()).toContainText("already registered");

  // A free one gets a code; the code adds the method.
  const email = uniqueEmail("dex");
  const before = await mailCount(page.request, email);
  await section.getByLabel("E-mail address").fill(email);
  await section.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await section.getByRole("button", { name: "Add e-mail address" }).click();
  await expect(page).toHaveURL(/\/auth\/email\/verify$/);
  await expect(page.getByText("We sent a 6-digit code to")).toContainText(email);
  await expect(page.getByRole("link", { name: "Back to your account" })).toBeVisible();
  await enterCode(page, await latestCode(page.request, email, before));
  await expect(page).toHaveURL(/\/auth\/account\?message=/);
  await expect(page.getByRole("status")).toContainText("E-mail address added");
  await expect(page.locator("section", { hasText: "Profile" })).toContainText(DEX_USER);
  await expect(section).toContainText(email);
  await expect(section).toContainText("cannot be changed");
  await expect(section.getByRole("button", { name: "Add e-mail address" })).toHaveCount(0);
  await expect(section.getByRole("button", { name: "Change password" })).toBeVisible();

  // The address now signs in, as the same user.
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await signInWithEmail(page, email);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(DEX_USER);
});
