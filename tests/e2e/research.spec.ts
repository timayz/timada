import { expect, test, type Page } from "@playwright/test";
import { FakeDataforseo, keywordsFor } from "./dataforseo";
import { registerWithEmail, uniqueEmail } from "./email";

// A niche study, end to end against a stand-in for the keyword source
// (dataforseo.ts): a user starts one from a few seed keywords, asks for the
// keywords, which are fetched off the page, and reads them back. A fetch the
// source refuses says why on the study; a study is its owner's only.

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

test("a user studies a niche from its keywords", async ({ page, browser }) => {
  const source = new FakeDataforseo();
  await source.start();
  try {
    await registerWithEmail(page, uniqueEmail("researcher"));
    await page.getByRole("link", { name: "Research", exact: true }).click();
    await expect(page).toHaveURL(/\/research$/);
    await expect(page.getByText("No study yet")).toBeVisible();

    // Started from two seeds, in a country and a language the source covers.
    await page.getByRole("link", { name: "Start a study" }).click();
    const name = `Cat trees ${Date.now()}`;
    await page.getByLabel("Name").fill(name);
    await page.getByLabel("Seed keywords, one a line").fill("Arbre à  Chat\nrefused niche\n");
    await expect(page.getByLabel("Searched in")).toHaveValue("FR:fr");
    await expect(page.getByLabel("Searched in").locator("option")).toHaveText(["France · French", "United States · English"]);
    await page.getByRole("button", { name: "Start the study" }).click();
    await expect(page.getByRole("status")).toContainText("Study started.");
    await expect(page.locator("[data-study-name]")).toHaveText(name);
    const studyUrl = page.url().split("?")[0];

    // Nothing is fetched until asked for.
    await expect(page.locator("[data-seed]")).toHaveCount(2);
    await expect(page.locator('[data-seed="arbre à chat"]')).toHaveAttribute("data-seed-status", "never");
    await expect(page.locator("[data-keyword]")).toHaveCount(0);
    expect(source.calls).toHaveLength(0);

    await page.getByRole("button", { name: "Fetch the keywords of 2 seeds" }).click();
    await expect(page.getByRole("status")).toContainText("The keywords are being fetched.");
    await seedStands(page, "arbre à chat", "fetched");
    await seedStands(page, "refused niche", "failed");
    expect(source.calls).toContainEqual({ keyword: "arbre à chat", locationCode: 2250, languageCode: "fr" });

    // What the source refused says why, and may be asked again.
    const refused = page.locator('[data-seed="refused niche"]');
    await expect(refused.locator("[data-seed-error]")).toContainText("The keyword source refused: Insufficient Funds.");
    await expect(refused.getByRole("button", { name: "Try again" })).toBeVisible();
    await expect(page.locator('[data-seed="arbre à chat"] [data-seed-keywords]')).toHaveText("5");

    // The keywords read back, the most searched first, a missing figure
    // left empty.
    await keywordsListed(page, 5);
    const expected = keywordsFor("arbre à chat");
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
    await page.getByLabel("A seed keyword to add").fill("Griffoir");
    await page.getByRole("button", { name: "Add the seed" }).click();
    await expect(page.getByRole("status")).toContainText("Seed added.");
    await page.locator('[data-seed="griffoir"]').getByRole("button", { name: "Fetch", exact: true }).click();
    await seedStands(page, "griffoir", "fetched");
    await keywordsListed(page, 10);

    // Fetched again, a seed's rows replace its earlier ones.
    await page.locator('[data-seed="arbre à chat"]').getByRole("button", { name: "Fetch again" }).click();
    await expect.poll(() => source.calls.filter((call) => call.keyword === "arbre à chat").length).toBe(2);
    await seedStands(page, "arbre à chat", "fetched");
    await keywordsListed(page, 10);

    await page.getByLabel("Name").fill(`${name} bis`);
    await page.getByRole("button", { name: "Rename" }).click();
    await expect(page.getByRole("status")).toContainText("Study renamed.");
    await expect(page.locator("[data-study-name]")).toHaveText(`${name} bis`);

    // The list counts its keywords.
    await expect(async () => {
      await page.goto("/research");
      await expect(page.locator(`[data-study="${name} bis"] [data-study-keywords]`)).toHaveText("10", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });

    // Nobody else opens it, or sees it in their list.
    const stranger = await (await browser.newContext()).newPage();
    await registerWithEmail(stranger, uniqueEmail("stranger"));
    expect((await stranger.goto(studyUrl))?.status()).toBe(404);
    await stranger.goto("/research");
    await expect(stranger.getByText("No study yet")).toBeVisible();

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
