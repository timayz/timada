import { expect, test, type CDPSession, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";

// Passkeys are added to an existing account (here one registered with an
// address) and then sign in on their own, without a mailed code.

// A CTAP2 platform authenticator with resident keys and user verification,
// as a phone or laptop with biometrics would behave. Chrome's WebAuthn
// virtual authenticator answers every ceremony without a prompt.
type Authenticator = { cdp: CDPSession; id: string };

async function virtualAuthenticator(page: Page, cdp?: CDPSession): Promise<Authenticator> {
  cdp ??= await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  return { cdp, id: authenticatorId };
}

test("add passkeys to an account, sign out, sign in without a username", async ({ page }) => {
  const email = uniqueEmail("alice");
  const first = await virtualAuthenticator(page);

  await page.goto("/");
  await expect(page).toHaveURL(/\/auth\/login$/);
  await registerWithEmail(page, email);
  await expect(page.getByText("Signed in as")).toContainText(email);

  await page.getByRole("link", { name: "Account" }).click();
  await expect(page.getByRole("heading", { name: "Passkeys" })).toBeVisible();
  await expect(page.locator("section", { hasText: "E-mail and password" })).toContainText(email);
  await expect(page.getByText("Administrator").or(page.getByText("Member"))).toBeVisible();
  const passkeyRows = page.locator("section", { hasText: "Passkeys" }).locator("li");
  await expect(passkeyRows).toHaveCount(0);

  // A ceremony is two requests, the authenticator and a navigation, so the
  // checks after a click get the suite's 15 s allowance for server round
  // trips. The button is disabled until auth.js has wired it, so the click
  // waits for the script.
  const ceremony = { timeout: 15_000 };
  await page.getByLabel("Name for the new passkey").fill("First key");
  await page.getByRole("button", { name: "Add a passkey" }).click();
  await expect(page.getByRole("status")).toContainText("Passkey added.", ceremony);
  await expect(passkeyRows).toHaveCount(1, ceremony);
  await expect(page.locator("section", { hasText: "Recent sign-ins" }).locator("li")).toHaveCount(1);

  // The first device already holds a credential for this account, so the
  // server's excludeCredentials makes it refuse a second one.
  await page.getByLabel("Name for the new passkey").fill("Second key");
  await page.getByRole("button", { name: "Add a passkey" }).click();
  await expect(page.locator("[data-auth-error]")).toContainText("already registered", ceremony);

  // A second device can add one, then the first passkey can go.
  await first.cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: first.id });
  await virtualAuthenticator(page, first.cdp);
  await page.getByRole("button", { name: "Add a passkey" }).click();
  // The previous page already says "Passkey added."; the second row is what
  // proves the new one landed.
  await expect(passkeyRows).toHaveCount(2, ceremony);
  await expect(page.getByRole("status")).toContainText("Passkey added.");
  await passkeyRows.first().getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("status")).toContainText("Passkey removed.");
  await expect(passkeyRows).toHaveCount(1);

  // Rename the remaining one.
  await passkeyRows.first().getByLabel("Passkey name").fill("Laptop");
  await passkeyRows.first().getByRole("button", { name: "Rename" }).click();
  await expect(page.getByRole("status")).toContainText("Passkey renamed.");
  await expect(passkeyRows.first().getByLabel("Passkey name")).toHaveValue("Laptop");

  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page).toHaveURL(/\/auth\/login$/);
  await page.goto("/auth/account");
  await expect(page).toHaveURL(/\/auth\/login$/);

  // Usernameless sign-in: the authenticator picks the resident credential,
  // and no code is needed.
  await page.getByRole("button", { name: "Sign in with a passkey" }).click();
  await expect(page).toHaveURL(/\/$/, ceremony);
  await expect(page.getByText("Signed in as")).toContainText(email);

  await page.goto("/auth/account");
  await expect(page.locator("section", { hasText: "Recent sign-ins" }).locator("li")).toHaveCount(2);
});

test("the session cookie is locked down and adding a passkey needs a session", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("bob"));
  await expect(page).toHaveURL(/\/$/);

  const cookies = await page.context().cookies();
  const session = cookies.find((c) => c.name === "__Host-session");
  expect(session?.secure).toBe(true);
  expect(session?.httpOnly).toBe(true);

  // A browser without a session cannot start a registration at all.
  const response = await request.post("/auth/passkey/register/start", {
    data: { user_id: "anyone" },
  });
  expect(response.status()).toBe(401);
  // A signed-in browser cannot start one for another user.
  const forged = await page.request.post("/auth/passkey/register/start", {
    data: { user_id: "someone-else" },
  });
  expect(forged.status()).toBe(403);
});
