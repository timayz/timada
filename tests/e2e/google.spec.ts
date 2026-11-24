import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { FakeGoogle, SEARCH_CONSOLE_SCOPE } from "./google";
import { createTenant, uniqueTenant } from "./tenants";

// A shop's connection to a Google account, end to end against a stand-in
// for Google (google.ts): a member says yes once on Google's page and comes
// back connected, chooses which of the account's Search Console sites is
// the shop's, and takes the connection off. An account that says no leaves
// the shop as it was, and a grant the account withdrew at Google shows as
// "connect again" until the member does, which keeps the site chosen.

test("a shop connects a Google account and chooses its Search Console site", async ({ page }) => {
  const google = new FakeGoogle();
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("google"));
    const shop = uniqueTenant("Google");
    await createTenant(page, shop.name);
    await page.goto(`/t/${shop.slug}/google`);
    await expect(page.getByRole("heading", { name: "Google account", level: 1 })).toBeVisible();
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "none");
    await expect(page.getByLabel("Search Console (read only)")).toBeChecked();

    // An account that says no: nothing is kept.
    google.refuse = true;
    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("alert")).toContainText("Google did not connect the account");
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "none");
    google.refuse = false;

    // One that unticks what was asked: nothing to read through.
    google.grants = (asked) => asked.filter((scope) => scope !== SEARCH_CONSOLE_SCOPE);
    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("alert")).toContainText("The account granted none of the access asked for");
    google.grants = undefined;

    // Yes: asked for the address and Search Console, to be renewed, every
    // time, on top of what the account granted before.
    await page.getByRole("button", { name: "Connect a Google account" }).click();
    await expect(page.getByRole("status")).toContainText("Google account connected.");
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/google\\?`));
    expect(google.asked.at(-1)).toEqual(["openid", "email", SEARCH_CONSOLE_SCOPE]);
    expect(google.visits.at(-1)).toMatchObject({
      response_type: "code",
      access_type: "offline",
      prompt: "consent",
      include_granted_scopes: "true",
    });
    expect(google.visits.at(-1)?.redirect_uri).toMatch(/\/google\/callback$/);
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "connected");
    await expect(page.locator("[data-google-account]")).toHaveText("merchant@example.com");
    await expect(page.locator("[data-google-accesses]")).toHaveText("Search Console (read only)");

    // The sites the account reads, without the one it never proved its own.
    await expect(page.locator("[data-google-site-missing]")).toBeVisible();
    await expect(page.getByRole("radio")).toHaveCount(2);
    await expect(page.getByRole("radio", { name: "https://unproved.example/" })).toHaveCount(0);
    await page.getByRole("radio", { name: "sc-domain:shop.example" }).check();
    await page.getByRole("button", { name: "Save the site" }).click();
    await expect(page.getByRole("status")).toContainText("Search Console site saved.");
    await expect(page.locator("[data-google-site]")).toHaveText("sc-domain:shop.example");
    // The list is asked of Google only to choose.
    await expect(page.getByRole("radio")).toHaveCount(0);
    // The access the grant came with served so far.
    expect(google.renewals).toBe(0);

    await page.getByRole("link", { name: "Change the site" }).click();
    await expect(page.getByRole("radio", { name: "sc-domain:shop.example" })).toBeChecked();
    await page.getByRole("radio", { name: "https://blog.shop.example/" }).check();
    await page.getByRole("button", { name: "Save the site" }).click();
    await expect(page.locator("[data-google-site]")).toHaveText("https://blog.shop.example/");

    // The account withdraws its yes at Google: the next read says so, and
    // it stays said.
    google.withdraw();
    await page.getByRole("link", { name: "Change the site" }).click();
    await expect(page.locator("[data-google-lapsed]")).toContainText("Google no longer honours this connection");
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "lapsed");
    await page.goto(`/t/${shop.slug}/google`);
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "lapsed");
    await expect(page.locator("[data-google-site]")).toHaveCount(0);

    // Connecting again with the same account keeps the site.
    await page.getByRole("button", { name: "Connect again" }).click();
    await expect(page.getByRole("status")).toContainText("Google account connected.");
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "connected");
    await expect(page.locator("[data-google-site]")).toHaveText("https://blog.shop.example/");

    // Another account sees other sites: the choice is to make again.
    google.account = "other@example.com";
    await page.getByRole("button", { name: "Connect again, or another account" }).click();
    await expect(page.locator("[data-google-account]")).toHaveText("other@example.com");
    await expect(page.locator("[data-google-site-missing]")).toBeVisible();

    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(page.getByRole("status")).toContainText("Google account disconnected.");
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "none");
    await expect(page.getByRole("button", { name: "Disconnect" })).toHaveCount(0);
  } finally {
    await google.stop();
  }
});
