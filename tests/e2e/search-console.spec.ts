import { expect, test, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { FakeGoogle, SEARCH_DAY, searchDay } from "./google";
import { createTenant, uniqueTenant } from "./tenants";

// What Search Console says of a shop's site, end to end against a stand-in
// for Google (google.ts): a shop that connected its account and chose its
// site is read off the page, for the days Google has finished, and the
// page gives the period's totals, the queries and the pages, each page told
// for what it is in the shop. A read Google does not answer says why and
// when it is tried again; a connection Google no longer honours says
// "connect again" and keeps what was read.

/// Connects the shop's Google account and chooses `site`.
async function connect(page: Page, slug: string, site: string) {
  await page.goto(`/t/${slug}/google`);
  await page.getByRole("button", { name: "Connect a Google account" }).click();
  await expect(page.getByRole("status")).toContainText("Google account connected.");
  await page.getByRole("radio", { name: site }).check();
  await page.getByRole("button", { name: "Save the site" }).click();
  await expect(page.getByRole("status")).toContainText("Search Console site saved.");
}

test("a shop reads its Search Console queries and pages", async ({ page }) => {
  const google = new FakeGoogle();
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("search"));
    const shop = uniqueTenant("Search");
    await createTenant(page, shop.name);

    // A category of the shop, which one of the pages found is.
    await page.goto(`/t/${shop.slug}/categories`);
    await page.getByLabel("Name").fill("Arbres a chat");
    await page.getByRole("button", { name: "Create category" }).click();
    await expect(page.getByRole("status")).toBeVisible();
    google.searchPages = [
      "https://shop.example/arbres-a-chat",
      "https://shop.example/",
      "https://shop.example/panier?step=2",
    ];

    // Nothing to read through yet.
    await page.getByRole("link", { name: "Search Console", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/search-console$`));
    await expect(page.locator("[data-search-console]")).toHaveAttribute("data-search-console", "not-connected");
    await page.locator("[data-search-console]").getByRole("link", { name: "Google account" }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/google$`));

    await connect(page, shop.slug, "sc-domain:shop.example");

    // Read off the page: the three days Google has finished.
    await page.goto(`/t/${shop.slug}/search-console`);
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-search-days]")).toHaveAttribute("data-search-days", "3", { timeout: 1000 });
    }).toPass({ timeout: 30_000 });
    await expect(page.locator("[data-search-console]")).toHaveAttribute("data-search-console", "read");
    await expect(page.locator("[data-search-clicks]")).toHaveAttribute("data-search-clicks", `${3 * SEARCH_DAY.clicks}`);
    await expect(page.locator("[data-search-impressions]")).toHaveAttribute(
      "data-search-impressions",
      `${3 * SEARCH_DAY.impressions}`,
    );
    await expect(page.locator("[data-search-rate]")).toHaveText("5.0 %");
    // 8.25, to one decimal.
    await expect(page.locator("[data-search-position]")).toHaveText("8.3");
    // The period ends on the last finished day, not today.
    await expect(page.locator("[data-search-period]")).toContainText(`to ${searchDay(2)}`);
    await expect(page.locator("[data-search-period]")).toContainText("sc-domain:shop.example");

    // One question for the days, then each day's queries and its pages,
    // never the two at once, all for the site chosen.
    expect(google.questions.every((question) => question.site === "sc-domain:shop.example")).toBe(true);
    expect(google.questions[0]).toMatchObject({ dimension: "date", first: searchDay(90) });
    expect(google.questions.filter((question) => question.dimension === "query")).toHaveLength(3);
    expect(google.questions.filter((question) => question.dimension === "page")).toHaveLength(3);
    expect(google.questions.filter((question) => question.dimension !== "date").every((q) => q.first === q.last)).toBe(
      true,
    );

    // The queries, the most clicked first.
    const queries = page.locator("[data-search-query]");
    await expect(queries).toHaveCount(2);
    await expect(queries.nth(0)).toHaveAttribute("data-search-query", "arbre à chat");
    await expect(queries.nth(0).locator("[data-clicks]")).toHaveAttribute("data-clicks", "18");
    await expect(queries.nth(0).locator("[data-position]")).toHaveText("4.0");
    await expect(queries.nth(1)).toHaveAttribute("data-search-query", "griffoir carton");
    await expect(queries.nth(1).locator("[data-position]")).toHaveText("12.5");

    // The pages, each told for what it is.
    const found = page.locator("[data-search-page]");
    await expect(found).toHaveCount(3);
    const category = page.locator('[data-search-page="/arbres-a-chat"]');
    await expect(category.locator("[data-search-kind]")).toHaveAttribute("data-search-kind", "category");
    await expect(category.locator("[data-search-kind]")).toHaveText("Category: Arbres a chat");
    await expect(category.locator("[data-clicks]")).toHaveAttribute("data-clicks", "15");
    await expect(page.locator('[data-search-page="/"] [data-search-kind]')).toHaveText("Home page");
    await expect(page.locator('[data-search-page="/panier"] [data-search-kind]')).toHaveText("Other");
    await expect(category.getByRole("link")).toHaveAttribute("href", "https://shop.example/arbres-a-chat");

    // A shorter period holds the same three days.
    await page.getByLabel("Period").selectOption("7");
    await page.getByRole("button", { name: "Show" }).click();
    await expect(page).toHaveURL(/days=7/);
    await expect(page.locator("[data-search-days]")).toHaveAttribute("data-search-days", "3");

    // Read once: reloading asks Google nothing more.
    const asked = google.questions.length;
    await page.reload();
    await page.reload();
    expect(google.questions).toHaveLength(asked);

    // The account withdraws its yes: the page says so once something met
    // the refusal, and keeps what was read.
    google.withdraw();
    await page.goto(`/t/${shop.slug}/google?sites=1`);
    await expect(page.locator("[data-google]")).toHaveAttribute("data-google", "lapsed");
    await page.goto(`/t/${shop.slug}/search-console`);
    await expect(page.locator("[data-search-console]")).toHaveAttribute("data-search-console", "lapsed");
    await expect(page.locator("[data-search-lapsed]")).toContainText("Google no longer honours the shop's connection");
    await expect(page.locator("[data-search-clicks]")).toHaveAttribute("data-search-clicks", "30");
    await page.locator("[data-search-lapsed]").getByRole("link", { name: "connect again" }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/google$`));
  } finally {
    await google.stop();
  }
});

test("a read Google does not answer says why and when it is tried again", async ({ page }) => {
  const google = new FakeGoogle();
  google.searchFails = true;
  await google.start();
  try {
    await registerWithEmail(page, uniqueEmail("search"));
    const shop = uniqueTenant("Search");
    await createTenant(page, shop.name);
    await connect(page, shop.slug, "https://blog.shop.example/");

    await page.goto(`/t/${shop.slug}/search-console`);
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-search-failure]")).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 30_000 });
    await expect(page.locator("[data-search-failure]")).toContainText(
      "The last read of Search Console failed: Google is unavailable: Backend Error. It is tried again from",
    );
    await expect(page.locator("[data-search-totals]")).toHaveCount(0);
    // The address of the site is one part of the path Google is asked at.
    expect(google.questions[0]).toMatchObject({ site: "https://blog.shop.example/", dimension: "date" });
    // Not asked again within the hour.
    const asked = google.questions.length;
    await page.waitForTimeout(3000);
    expect(google.questions).toHaveLength(asked);
  } finally {
    await google.stop();
  }
});
