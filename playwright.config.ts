import { defineConfig, devices } from "@playwright/test";

// End-to-end tests drive a server that is already running behind the Traefik
// dev proxy (`make up` + `make dev`, or the smoke script). The `__Host-`
// session cookie needs HTTPS, so tests go through https://timada.localhost
// rather than the plain-HTTP port. Override with E2E_BASE_URL.
export default defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  fullyParallel: false,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: process.env.E2E_BASE_URL ?? "https://timada.localhost",
    ignoreHTTPSErrors: true,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
