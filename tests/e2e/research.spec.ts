import { expect, test, type Page } from "@playwright/test";
import { FakeDataforseo, keywordsFor } from "./dataforseo";
import { registerWithEmail, uniqueEmail } from "./email";
import { ADMIN_STATE } from "./fixtures";

// A niche study, end to end against a stand-in for the keyword source
// (dataforseo.ts): a user starts one from a few seed keywords, asks for the
// keywords, which are fetched off the page, and reads them back. A fetch the
// source refuses says why on the study; a study is its owner's only.
//
// Every call is paid by the platform: a user's day is capped (config/e2e.toml
// leaves room for three calls), a seed fetched before answers whoever asks
// for it again without a call, and root reads what each user spent. The
// seeds carry a tag of their own, so a run never reuses another's fetches.

/// Reloads the study's page until the seed's fetch stands at `status`.
async function seedStands(page: Page, seed: string, status: string) {
  await expect(async () => {
    await page.reload();
    await expect(page.locator(`[data-seed="${seed}"]`)).toHaveAttribute("data-seed-status", status, { timeout: 1000 });
  }).toPass({ timeout: 20_000 });
}

/// Reloads the study's page until it lists `count` keywords: the list comes
/// from the read model, a moment behind the study's own stream.
async function keywordsListed(page: Page, count: number) {
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-keyword]")).toHaveCount(count, { timeout: 1000 });
  }).toPass({ timeout: 20_000 });
}

/// Reloads the study's page until it says the user's fetches cost `micros`
/// (millionths of a dollar) today.
async function spentToday(page: Page, micros: number) {
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-spent-today]")).toHaveAttribute("data-spent-today", `${micros}`, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
}

test("a user studies a niche from its keywords", async ({ page, browser }) => {
  const source = new FakeDataforseo();
  await source.start();
  const tag = Date.now().toString(36);
  const cats = `arbre à chat ${tag}`;
  const refusedSeed = `refused niche ${tag}`;
  const scratcher = `griffoir ${tag}`;
  const callsFor = (seed: string) => source.calls.filter((call) => call.keyword === seed).length;
  try {
    const researcher = uniqueEmail("researcher");
    await registerWithEmail(page, researcher);
    await page.getByRole("link", { name: "Research", exact: true }).click();
    await expect(page).toHaveURL(/\/research$/);
    await expect(page.getByText("No study yet")).toBeVisible();

    // Started from two seeds, in a country and a language the source covers.
    await page.getByRole("link", { name: "Start a study" }).click();
    const name = `Cat trees ${Date.now()}`;
    await page.getByLabel("Name").fill(name);
    await page.getByLabel("Seed keywords, one a line").fill(`Arbre à  Chat ${tag}\n${refusedSeed}\n`);
    await expect(page.getByLabel("Searched in")).toHaveValue("FR:fr");
    await expect(page.getByLabel("Searched in").locator("option")).toHaveText(["France · French", "United States · English"]);
    await page.getByRole("button", { name: "Start the study" }).click();
    await expect(page.getByRole("status")).toContainText("Study started.");
    await expect(page.locator("[data-study-name]")).toHaveText(name);
    const studyUrl = page.url().split("?")[0];

    // Nothing is fetched until asked for.
    await expect(page.locator("[data-seed]")).toHaveCount(2);
    await expect(page.locator(`[data-seed="${cats}"]`)).toHaveAttribute("data-seed-status", "never");
    await expect(page.locator("[data-keyword]")).toHaveCount(0);
    expect(source.calls).toHaveLength(0);

    await page.getByRole("button", { name: "Fetch the keywords of 2 seeds" }).click();
    await expect(page.getByRole("status")).toContainText("The keywords are being fetched.");
    await seedStands(page, cats, "fetched");
    await seedStands(page, refusedSeed, "failed");
    expect(source.calls).toContainEqual({ keyword: cats, locationCode: 2250, languageCode: "fr" });

    // What the source refused says why, and may be asked again.
    const refused = page.locator(`[data-seed="${refusedSeed}"]`);
    await expect(refused.locator("[data-seed-error]")).toContainText("The keyword source refused: Insufficient Funds.");
    await expect(refused.getByRole("button", { name: "Try again" })).toBeVisible();
    await expect(page.locator(`[data-seed="${cats}"] [data-seed-keywords]`)).toHaveText("5");
    // What the call cost counts to the user's day; the refused one cost
    // nothing.
    await spentToday(page, 12480);
    await expect(page.locator("[data-spent-today]")).toContainText("$0.0125 today (UTC), of the $0.16 a day may cost");

    // The keywords read back, the most searched first, a missing figure
    // left empty.
    await keywordsListed(page, 5);
    const expected = keywordsFor(cats);
    await expect(page.locator("[data-keyword]").first()).toHaveAttribute("data-keyword", expected[0].keyword);
    const first = page.locator(`[data-keyword="${expected[0].keyword}"]`);
    await expect(first.locator("[data-volume]")).toHaveText("12100");
    await expect(first.locator("[data-cpc]")).toHaveText("$0.42");
    await expect(first.locator("[data-difficulty]")).toHaveText("38");
    await expect(first.locator("[data-intent]")).toHaveText("commercial");
    const unpriced = page.locator(`[data-keyword="${expected[4].keyword}"]`);
    await expect(unpriced.locator("[data-cpc]")).toHaveText("–");
    await expect(unpriced.locator("[data-difficulty]")).toHaveText("–");

    // A seed added later is fetched on its own.
    const addSeed = async (seed: string, typed = seed) => {
      await page.getByLabel("A seed keyword to add").fill(typed);
      await page.getByRole("button", { name: "Add the seed" }).click();
      await expect(page.getByRole("status")).toContainText("Seed added.");
      await page.locator(`[data-seed="${seed}"]`).getByRole("button", { name: "Fetch", exact: true }).click();
    };
    await addSeed(scratcher, `Griffoir ${tag}`);
    await seedStands(page, scratcher, "fetched");
    await keywordsListed(page, 10);

    // A seed just fetched is not offered again: asked for, it would be
    // answered with the keywords it has.
    await expect(page.locator(`[data-seed="${cats}"]`).getByRole("button")).toHaveCount(0);
    await expect(page.locator(`[data-seed="${refusedSeed}"]`).getByRole("button", { name: "Try again" })).toBeVisible();

    // The third call is the last the user's day has room for: the page
    // says which cap is reached and when it frees, and the next fetch is
    // refused before the source is asked.
    const litter = `litière ${tag}`;
    const bowl = `gamelle ${tag}`;
    await addSeed(litter);
    await seedStands(page, litter, "fetched");
    await spentToday(page, 37440);
    const capped = page.locator('[data-spend-capped="user"]');
    await expect(capped).toContainText("You have reached the most keyword fetches may cost in a day. It frees on");
    await expect(capped).toContainText("00:00:00 UTC");
    await expect(capped).toContainText("A seed fetched in the last 30 days is still answered.");
    await addSeed(bowl);
    await seedStands(page, bowl, "failed");
    await expect(page.locator(`[data-seed="${bowl}"] [data-seed-error]`)).toContainText(
      "You have reached the most keyword fetches may cost in a day. It frees on",
    );
    expect(callsFor(bowl)).toBe(0);
    expect(callsFor(cats)).toBe(1);
    await keywordsListed(page, 15);

    await page.getByLabel("Name").fill(`${name} bis`);
    await page.getByRole("button", { name: "Rename" }).click();
    await expect(page.getByRole("status")).toContainText("Study renamed.");
    await expect(page.locator("[data-study-name]")).toHaveText(`${name} bis`);

    // The list counts its keywords.
    await expect(async () => {
      await page.goto("/research");
      await expect(page.locator(`[data-study="${name} bis"] [data-study-keywords]`)).toHaveText("15", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });

    // Nobody else opens it, or sees it in their list.
    const stranger = await (await browser.newContext()).newPage();
    await registerWithEmail(stranger, uniqueEmail("stranger"));
    expect((await stranger.goto(studyUrl))?.status()).toBe(404);
    await stranger.goto("/research");
    await expect(stranger.getByText("No study yet")).toBeVisible();

    // The same seed, country and language asked by someone else is
    // answered with the keywords already fetched: no call, no cost.
    await stranger.getByRole("link", { name: "Start a study" }).click();
    await stranger.getByLabel("Name").fill(`Same niche ${tag}`);
    await stranger.getByLabel("Seed keywords, one a line").fill(cats);
    await stranger.getByRole("button", { name: "Start the study" }).click();
    await expect(stranger.getByRole("status")).toContainText("Study started.");
    await stranger.getByRole("button", { name: "Fetch the keywords of 1 seed" }).click();
    await seedStands(stranger, cats, "fetched");
    await keywordsListed(stranger, 5);
    expect(callsFor(cats)).toBe(1);
    await spentToday(stranger, 0);

    // Root reads what the day and the month cost, by user.
    const admin = await (await browser.newContext({ storageState: ADMIN_STATE })).newPage();
    await admin.goto("/");
    await admin.getByRole("link", { name: "Research spend" }).click();
    await expect(admin).toHaveURL(/\/admin\/research$/);
    const spender = admin.locator("[data-spend-user]", { hasText: researcher });
    await expect(async () => {
      await admin.reload();
      await expect(spender.locator("[data-spend-user-day]")).toHaveAttribute("data-spend-user-day", "37440", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(spender.locator("[data-spend-user-day]")).toHaveText("$0.0374");
    await expect(spender.locator("[data-spend-user-month]")).toHaveAttribute("data-spend-user-month", "37440");
    expect((await stranger.goto("/admin/research"))?.status()).toBe(403);

    // Deleted, it is gone.
    await page.goto(studyUrl);
    await page.getByRole("button", { name: "Delete the study" }).click();
    await expect(page.getByRole("status")).toContainText("Study deleted.");
    await expect(async () => {
      await page.goto("/research");
      await expect(page.getByText("No study yet")).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    expect((await page.goto(studyUrl))?.status()).toBe(404);
  } finally {
    await source.stop();
  }
});

test("a study needs a seed and a place the source covers", async ({ page }) => {
  const source = new FakeDataforseo();
  await source.start();
  try {
    await registerWithEmail(page, uniqueEmail("researcher"));
    await page.goto("/research/new");
    await page.getByLabel("Name").fill("Nowhere");
    await page.getByLabel("Seed keywords, one a line").fill("   ");
    await page.getByRole("button", { name: "Start the study" }).click();
    await expect(page).toHaveURL(/error=study_without_seed/);
    await expect(page.getByRole("alert").first()).toContainText("one seed keyword at least");

    // A place typed past the list is refused.
    await page.getByLabel("Name").fill("Nowhere");
    await page.getByLabel("Seed keywords, one a line").fill("griffoir");
    await page.getByLabel("Searched in").evaluate((select: HTMLSelectElement) => {
      select.options[0].value = "ZZ:zz";
      select.selectedIndex = 0;
    });
    await page.getByRole("button", { name: "Start the study" }).click();
    await expect(page).toHaveURL(/error=invalid_study_place/);
  } finally {
    await source.stop();
  }
});
