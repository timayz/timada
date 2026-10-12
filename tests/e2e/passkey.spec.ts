import { expect, test, type CDPSession, type Page } from "@playwright/test";

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

const name = `Alice ${Date.now()}`;

test("register with a passkey, sign out, sign in without a username", async ({ page }) => {
  const first = await virtualAuthenticator(page);

  await page.goto("/");
  await expect(page).toHaveURL(/\/auth\/login$/);

  await page.getByText("New here? Create an account with a passkey").click();
  await page.getByLabel("Your name").fill(name);
  await page.getByRole("button", { name: "Create account" }).click();

  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByText(`Signed in as`)).toContainText(name);

  await page.getByRole("link", { name: "Account" }).click();
  await expect(page.getByRole("heading", { name: "Passkeys" })).toBeVisible();
  const passkeyRows = page.locator("section", { hasText: "Passkeys" }).locator("li");
  await expect(passkeyRows).toHaveCount(1);
  await expect(page.getByText("Administrator").or(page.getByText("Member"))).toBeVisible();
  await expect(page.locator("section", { hasText: "Recent sign-ins" }).locator("li")).toHaveCount(1);

  // The only sign-in method cannot be removed.
  await page.getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("alert")).toContainText("only way to sign in");

  // The first device already holds a credential for this account, so the
  // server's excludeCredentials makes it refuse a second one.
  await page.getByLabel("Name for the new passkey").fill("Second key");
  await page.getByRole("button", { name: "Add a passkey" }).click();
  await expect(page.locator("[data-auth-error]")).toContainText("already registered");

  // A second device can add one, then the first passkey can go.
  await first.cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: first.id });
  await virtualAuthenticator(page, first.cdp);
  await page.getByRole("button", { name: "Add a passkey" }).click();
  await expect(page.getByRole("status")).toContainText("Passkey added.");
  await expect(passkeyRows).toHaveCount(2);
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

  // Usernameless sign-in: the authenticator picks the resident credential.
  await page.getByRole("button", { name: "Sign in with a passkey" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByText("Signed in as")).toContainText(name);

  await page.goto("/auth/account");
  await expect(page.locator("section", { hasText: "Recent sign-ins" }).locator("li")).toHaveCount(2);
});

test("a signed-in browser cannot start a registration for a new user", async ({ page, request }) => {
  await virtualAuthenticator(page);
  await page.goto("/auth/login");
  await page.getByText("New here? Create an account with a passkey").click();
  await page.getByLabel("Your name").fill(`Bob ${Date.now()}`);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/$/);

  const cookies = await page.context().cookies();
  const session = cookies.find((c) => c.name === "__Host-session");
  expect(session?.secure).toBe(true);
  expect(session?.httpOnly).toBe(true);

  const response = await page.request.post("/auth/passkey/register/start", {
    data: { mode: "create_user", name: "Mallory" },
  });
  expect(response.status()).toBe(400);
  expect(await response.text()).toContain("already signed in");
});
