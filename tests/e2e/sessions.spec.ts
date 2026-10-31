import { expect, test, type BrowserContext, type Page } from "@playwright/test";
import { registerWithEmail, signInWithEmail, uniqueEmail } from "./email";
import { virtualAuthenticator } from "./webauthn";

// Sessions across browsers: "sign out everywhere else" and switching accounts
// by signing in with another user's passkey while signed in.

/// Adds a passkey to the signed-in account from the account page.
async function addPasskey(page: Page) {
  await page.goto("/auth/account");
  await page.getByRole("button", { name: "Add a passkey" }).click();
  await expect(page.getByRole("status")).toContainText("Passkey added.");
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
  const email = uniqueEmail("alice");

  await registerWithEmail(laptopPage, email);
  await signInWithEmail(phonePage, email);
  await expect(phonePage.locator("header").getByText("Signed in as")).toBeVisible();

  await laptopPage.goto("/auth/account");
  await laptopPage.getByRole("button", { name: "Sign out everywhere else" }).click();
  await expect(laptopPage.getByRole("status")).toContainText("Signed out everywhere else.");

  await phonePage.goto("/auth/account");
  await expect(phonePage).toHaveURL(/\/auth\/login$/);
  await laptopPage.goto("/");
  await expect(laptopPage.locator("header").getByText("Signed in as")).toBeVisible();

  // The phone can sign in again right away.
  await signInWithEmail(phonePage, email);
  await expect(phonePage.locator("header").getByText("Signed in as")).toBeVisible();

  await laptop.close();
  await phone.close();
});

test("signing in with another user's passkey switches accounts", async ({ page }) => {
  const alice = uniqueEmail("alice");
  const bob = uniqueEmail("bob");
  await virtualAuthenticator(page);
  await registerWithEmail(page, alice);
  await addPasskey(page);
  await page.getByRole("button", { name: "Sign out", exact: true }).click();

  // Bob registers in the same browser; it is his now. He creates no passkey,
  // so the authenticator still only holds Alice's.
  await registerWithEmail(page, bob);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(bob);

  // Alice's passkey is used while Bob is signed in.
  const location = await passkeySignInViaApi(page);
  await page.goto(location);
  await expect(page.getByRole("status")).toContainText(`You are now signed in as ${alice}.`);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(alice);

  // Her own passkey again: no switch, no notice.
  const again = await passkeySignInViaApi(page);
  expect(again).not.toContain("message=");
  await page.goto(again);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(alice);
});
