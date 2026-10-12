import { test as setup } from "@playwright/test";
import { registerWithEmail } from "./email";
import { ADMIN_EMAIL, ADMIN_PASSWORD, ADMIN_STATE } from "./fixtures";

// Runs before every other test: registers the first user, who administers
// the instance, and saves their signed-in state for the admin tests.

setup("register the administrator", async ({ page }) => {
  await registerWithEmail(page, ADMIN_EMAIL, ADMIN_PASSWORD);
  await page.context().storageState({ path: ADMIN_STATE });
});
