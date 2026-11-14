import { SHARD } from "./shard";

// Shared by admin.setup.ts and the specs that reuse its signed-in state.
/// The administrator's signed-in state on server `shard`.
export const adminState = (shard: number) => `test-results/.auth/admin-${shard}.json`;
/// The administrator's signed-in state on this worker's server.
export const ADMIN_STATE = adminState(SHARD);
export const ADMIN_EMAIL = "admin-e2e@example.com";
export const ADMIN_PASSWORD = "admin e2e password";
// Accounts created with an address are labelled with it.
export const ADMIN_NAME = ADMIN_EMAIL;
