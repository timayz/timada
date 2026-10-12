import { expect, test as setup } from "@playwright/test";
import { ADMIN_NAME, ADMIN_STATE } from "./fixtures";

// Runs before every other test: registers the first user, who administers
// the instance, and saves their signed-in state for the admin tests.

setup("register the administrator", async ({ page }) => {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  await page.goto("/auth/login");
  await page.getByText("New here? Create an account with a passkey").click();
  await page.getByLabel("Your name").fill(ADMIN_NAME);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/$/);
  await page.context().storageState({ path: ADMIN_STATE });
});
