import { test as setup } from "@playwright/test";
import { registerWithEmail } from "./email";
import { ADMIN_EMAIL, ADMIN_PASSWORD, adminState } from "./fixtures";
import { SHARDS, shardUrl } from "./shard";

// Runs before every other test: registers the first user of each server, who
// administers it, and saves their signed-in state for the admin tests. One
// server after the other, as the code to enter is the newest one mailed to
// the address they share.

setup("register the administrator", async ({ browser }) => {
  for (let shard = 0; shard < SHARDS; shard++) {
    const context = await browser.newContext({ baseURL: shardUrl(shard) });
    await registerWithEmail(await context.newPage(), ADMIN_EMAIL, ADMIN_PASSWORD);
    await context.storageState({ path: adminState(shard) });
    await context.close();
  }
});
