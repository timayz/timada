import { SHARD } from "./shard";

// Shared by admin.setup.ts and the specs that reuse its signed-in state. The
// administrator of the suite is the root account of config/e2e.toml.
/// Root's signed-in state on server `shard`.
export const adminState = (shard: number) => `test-results/.auth/admin-${shard}.json`;
/// Root's signed-in state on this worker's server.
export const ADMIN_STATE = adminState(SHARD);
export const ADMIN_EMAIL = "root-e2e@example.com";
export const ADMIN_PASSWORD = "root e2e password";
// Root is labelled with its address.
export const ADMIN_NAME = ADMIN_EMAIL;
