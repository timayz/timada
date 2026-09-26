import { defineConfig, devices } from "@playwright/test";

// Everything Playwright needs comes from the dev shell (`nix develop`): the
// CLI, and the browsers under PLAYWRIGHT_BROWSERS_PATH. There is no
// package.json and no node_modules on purpose — this repository is Rust.

const port = Number(process.env.TIMADA_E2E_PORT ?? 3001);

// Point the run at a shop that is already up (`topcoat dev -p demo` serves on
// :3000) instead of letting Playwright start one:
//   TIMADA_E2E_BASE_URL=http://127.0.0.1:3000 playwright test
const external = process.env.TIMADA_E2E_BASE_URL;
const baseURL = external ?? `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  reporter: [["list"]],

  use: {
    baseURL,
    trace: "on-first-retry",
  },

  // Chromium only: the dev shell deliberately leaves firefox and webkit out of
  // PLAYWRIGHT_BROWSERS_PATH (see flake.nix), so naming them here would fail
  // with "Executable doesn't exist".
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],

  webServer: external
    ? undefined
    : {
        // The demo keeps its database at the fixed path data/demo.db, so seed
        // only when there is nothing there yet — re-seeding an existing shop
        // places another order every time.
        command:
          "sh -c 'if [ ! -f data/demo.db ]; then cargo run -q -p demo -- --seed; fi; exec cargo run -q -p demo'",
        url: baseURL,
        env: { PORT: String(port) },
        // A cold `cargo build` of the workspace is slow.
        timeout: 15 * 60 * 1000,
        reuseExistingServer: true,
        stdout: "pipe",
        stderr: "pipe",
      },
});
