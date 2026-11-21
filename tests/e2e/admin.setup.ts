import { expect, test as setup } from "@playwright/test";
import { ADMIN_EMAIL, ADMIN_PASSWORD, adminState } from "./fixtures";
import { SHARDS, shardUrl } from "./shard";

// Runs before every other test: signs in as the root account of each server
// (config/e2e.toml), which administers it, and saves the signed-in state for
// the admin tests. Root's password signs it in at once: no code is mailed.

setup("sign in as root", async ({ browser }) => {
  for (let shard = 0; shard < SHARDS; shard++) {
    const context = await browser.newContext({ baseURL: shardUrl(shard) });
    const page = await context.newPage();
    await page.goto("/auth/login");
    await page.getByLabel("E-mail address").fill(ADMIN_EMAIL);
    await page.getByLabel("Password", { exact: true }).fill(ADMIN_PASSWORD);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/$/);
    await context.storageState({ path: adminState(shard) });
    await context.close();
  }
});
