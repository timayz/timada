import { expect, test, type BrowserContext, type CDPSession, type Page } from "@playwright/test";

// Sessions across browsers: "sign out everywhere else" and switching accounts
// by signing in with another user's passkey while signed in.

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

async function registerWithPasskey(page: Page, name: string) {
  await page.goto("/auth/login");
  await page.getByText("New here? Create an account with a passkey").click();
  await page.getByLabel("Your name").fill(name);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(name);
}

async function signInWithPasskey(page: Page) {
  await page.goto("/auth/login");
  await page.getByRole("button", { name: "Sign in with a passkey" }).click();
  await expect(page).toHaveURL(/\/$/);
}

/// Moves every credential of `from` into a fresh authenticator on `to`, so a
/// second browser context can sign in with a passkey created in the first.
async function copyCredentials(from: Authenticator, to: Authenticator) {
  const { credentials } = await from.cdp.send("WebAuthn.getCredentials", { authenticatorId: from.id });
  for (const credential of credentials) {
    await to.cdp.send("WebAuthn.addCredential", { authenticatorId: to.id, credential });
  }
}

/// The browser-side half of a passkey sign-in, as auth.js does it, without
/// the login page (which redirects signed-in browsers home).
async function passkeySignInViaApi(page: Page): Promise<string> {
  return page.evaluate(async () => {
    const post = async (url: string, body: unknown) => {
      const response = await fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify(body),
      });
      if (!response.ok) throw new Error(await response.text());
      return response.json();
    };
    const options = await post("/auth/passkey/login/start", {});
    const credential = (await navigator.credentials.get({
      // @ts-expect-error WebAuthn Level 3 helper, present in Chromium.
      publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(options.publicKey),
    })) as PublicKeyCredential;
    // @ts-expect-error toJSON is WebAuthn Level 3.
    const done = await post("/auth/passkey/login/finish", credential.toJSON());
    return done.location as string;
  });
}

test("sign out everywhere else keeps this browser and ends the others", async ({ browser }) => {
  const laptop: BrowserContext = await browser.newContext();
  const phone: BrowserContext = await browser.newContext();
  const laptopPage = await laptop.newPage();
  const phonePage = await phone.newPage();
  const laptopAuth = await virtualAuthenticator(laptopPage);
  const phoneAuth = await virtualAuthenticator(phonePage);

  await registerWithPasskey(laptopPage, `Alice ${Date.now()}`);
  await copyCredentials(laptopAuth, phoneAuth);
  await signInWithPasskey(phonePage);
  await expect(phonePage.locator("header").getByText("Signed in as")).toBeVisible();

  await laptopPage.goto("/auth/account");
  await laptopPage.getByRole("button", { name: "Sign out everywhere else" }).click();
  await expect(laptopPage.getByRole("status")).toContainText("Signed out everywhere else.");

  await phonePage.goto("/auth/account");
  await expect(phonePage).toHaveURL(/\/auth\/login$/);
  await laptopPage.goto("/");
  await expect(laptopPage.locator("header").getByText("Signed in as")).toBeVisible();

  // The phone can sign in again right away.
  await signInWithPasskey(phonePage);
  await expect(phonePage.locator("header").getByText("Signed in as")).toBeVisible();

  await laptop.close();
  await phone.close();
});

test("signing in with another user's passkey switches accounts", async ({ page }) => {
  const stamp = Date.now();
  const first = await virtualAuthenticator(page);
  await registerWithPasskey(page, `Alice ${stamp}`);
  const { credentials: aliceCredentials } = await first.cdp.send("WebAuthn.getCredentials", {
    authenticatorId: first.id,
  });
  await page.getByRole("button", { name: "Sign out", exact: true }).click();

  // Bob registers on another device; the browser is now his.
  await first.cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: first.id });
  const second = await virtualAuthenticator(page, first.cdp);
  await registerWithPasskey(page, `Bob ${stamp}`);

  // Alice's key is plugged back in and used while Bob is signed in.
  await first.cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: second.id });
  const third = await virtualAuthenticator(page, first.cdp);
  for (const credential of aliceCredentials) {
    await third.cdp.send("WebAuthn.addCredential", { authenticatorId: third.id, credential });
  }
  const location = await passkeySignInViaApi(page);
  await page.goto(location);
  await expect(page.getByRole("status")).toContainText(`You are now signed in as Alice ${stamp}.`);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(`Alice ${stamp}`);

  // Her own passkey again: no switch, no notice.
  const again = await passkeySignInViaApi(page);
  expect(again).not.toContain("message=");
  await page.goto(again);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(`Alice ${stamp}`);
});
