import { defineConfig, devices } from "@playwright/test";
import { SHARD, SHARDS, shardUrl } from "./tests/e2e/shard";

// End-to-end tests drive servers that are already running behind the Traefik
// dev proxy: `make e2e` starts E2E_SHARDS of them on fresh databases and sets
// E2E_SHARDS, which gives each worker a server of its own (tests/e2e/shard.ts),
// at https://s<N>.timada.localhost. Without it the tests run one at a time
// against https://timada.localhost, as `make e2e.cluster` does with its three
// nodes (it sets E2E_NODES=3, which the specs read where the number of nodes
// shows), or against `make dev` (the admin tests then need the first registered
// user to be the one `setup` creates, so they only pass on a fresh database).
// The `__Host-` session cookie needs HTTPS, so tests go through the proxy
// rather than the plain-HTTP port. Override with E2E_BASE_URL.
export default defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  // A spec file runs whole on one worker, and so on one server.
  fullyParallel: false,
  workers: SHARDS,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: shardUrl(SHARD),
    ignoreHTTPSErrors: true,
    trace: "retain-on-failure",
  },
  projects: [
    // Registers the administrator (the first user) before anything else can.
    { name: "setup", testMatch: /.*\.setup\.ts/ },
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
      dependencies: ["setup"],
      testIgnore: /.*\.setup\.ts/,
    },
  ],
});
