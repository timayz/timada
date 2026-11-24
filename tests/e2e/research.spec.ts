import { expect, test, type Page } from "@playwright/test";
import { FakeDataforseo, keywordsFor, sitesFor } from "./dataforseo";
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
//
// A study is scored from its keywords and from what its owner says of it:
// the page gives the verdict with the tests behind it, what one sale leaves,
// the keywords by what the score made of each, and the owner corrects it
// (a word excluded, a keyword dropped or brought back) without a fetch. The
// list sets the studies side by side, and the keywords export with their
// score.
//
// Who holds the first results of the main keywords is searched off the page
// too, one paid search a keyword: the page lists the sites found, the owner
// marks each as a specialist shop or a generalist, and the competition test
// follows. A mark is the user's: the next study knows it.
//
// The kept keywords are what a shop's categories are named after: the owner
// builds a tree from them by hand, parents, children and keywords parked
// for later, and the page says what stands between it and its creation.

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
    await expect(capped).toContainText("You have reached the most keyword research may cost in a day. It frees on");
    await expect(capped).toContainText("00:00:00 UTC");
    await expect(capped).toContainText("A seed fetched in the last 30 days is still answered.");
    await addSeed(bowl);
    await seedStands(page, bowl, "failed");
    await expect(page.locator(`[data-seed="${bowl}"] [data-seed-error]`)).toContainText(
      "You have reached the most keyword research may cost in a day. It frees on",
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

/// Reloads the study's page until the test `key` of its verdict stands at
/// `state`: what the keywords come to is the read model's, a moment behind.
async function testStands(page: Page, key: string, state: string) {
  await expect(async () => {
    await page.reload();
    await expect(page.locator(`[data-test="${key}"]`)).toHaveAttribute("data-test-state", state, { timeout: 1000 });
  }).toPass({ timeout: 20_000 });
}

/// Opens one of the keyword lists and reloads it until it holds `keywords`,
/// the most searched first.
async function listHolds(page: Page, list: RegExp, keywords: string[]) {
  await page.getByRole("navigation", { name: "Keyword lists" }).getByRole("link", { name: list }).click();
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-keyword]")).toHaveCount(keywords.length, { timeout: 1000 });
    for (const [at, keyword] of keywords.entries()) {
      await expect(page.locator("[data-keyword]").nth(at)).toHaveAttribute("data-keyword", keyword, { timeout: 1000 });
    }
  }).toPass({ timeout: 20_000 });
}

test("a study is scored, corrected by its owner and compared", async ({ page, browser }) => {
  const source = new FakeDataforseo();
  await source.start();
  const tag = Date.now().toString(36);
  const seed = `griffoir ${tag}`;
  const cheap = `${seed} pas cher`;
  const best = `meilleur ${seed}`;
  try {
    await registerWithEmail(page, uniqueEmail("scorer"));
    await page.goto("/research/new");
    const name = `Scratchers ${Date.now()}`;
    await page.getByLabel("Name").fill(name);
    await page.getByLabel("Seed keywords, one a line").fill(seed);
    await page.getByRole("button", { name: "Start the study" }).click();
    await expect(page.getByRole("status")).toContainText("Study started.");
    const studyUrl = page.url().split("?")[0];

    // Nothing fetched, nothing said: every test waits.
    await expect(page.locator("[data-verdict]")).toHaveAttribute("data-verdict", "incomplete");
    await expect(page.locator('[data-test-state="not_checked"]')).toHaveCount(6);
    await expect(page.locator("[data-sale-unknown]")).toBeVisible();

    await page.getByRole("button", { name: "Fetch the keywords of 1 seed" }).click();
    await seedStands(page, seed, "fetched");
    await keywordsListed(page, 5);

    // The fetch scored it: two main keywords are not three. The seed's own
    // is too difficult, two ask a question.
    await testStands(page, "main_keywords", "failed");
    await expect(page.locator("[data-verdict]")).toHaveAttribute("data-verdict", "no_go");
    await expect(page.locator('[data-test="main_keywords"] [data-test-numbers]')).toContainText("2 kept");
    await expect(page.locator('[data-test="margin"]')).toHaveAttribute("data-test-state", "not_checked");
    await listHolds(page, /^Main/, [cheap, best]);
    await listHolds(page, /^Informational/, [`${seed} avis`, `fabriquer un ${seed}`]);
    await listHolds(page, /^Too difficult/, [seed]);

    // Brought back by hand, it is kept whatever its difficulty.
    await page.locator(`[data-keyword="${seed}"]`).getByRole("button", { name: "Bring back" }).click();
    await expect(page.getByRole("status")).toContainText("Keyword brought back.");
    await expect(page).toHaveURL(/list=too_difficult/);
    await listHolds(page, /^Main/, [seed, cheap, best]);
    await testStands(page, "main_keywords", "passed");
    await testStands(page, "long_tail", "failed");

    // What the owner says of the niche scores it again; no call is made.
    const calls = source.calls.length;
    await page.getByLabel("Average price, VAT included").fill("60");
    await page.getByLabel("Supplier's cost").fill("15");
    await page.getByLabel("Sending a parcel").fill("4");
    await page.getByLabel("At the border").fill("5");
    await page.getByLabel("VAT in the price, in %").fill("20");
    await page.getByLabel("Fixed costs, for each sale").fill("1");
    await page.getByLabel("Payment fee, in % of the price").fill("1.5");
    await page.getByLabel("Payment fee, for each payment").fill("0.25");
    await page.getByLabel("Words to exclude, one a line").fill("Pas  cher\n");
    await page.getByLabel("Good products found at the supplier").fill("240");
    await page.getByRole("button", { name: "Save and score again" }).click();
    await expect(page.getByRole("status")).toContainText("Saved: the study is scored again.");

    // One sale, line by line: read from the study's own stream, at once.
    await expect(page.locator('[data-sale-line="margin"]')).toContainText("23.85");
    await expect(page.locator('[data-sale-line="break-even"]')).toHaveText("2.52");
    await expect(page.locator('[data-sale-line="needed"]')).toContainText("194.67");
    await expect(page.locator('[data-sale-line="highest-cpc"]')).toHaveText("0.308 EUR");
    await expect(page.locator('[data-test="margin"]')).toHaveAttribute("data-test-state", "passed");
    await expect(page.locator('[data-test="supplier_products"]')).toHaveAttribute("data-test-state", "passed");
    await expect(page.locator('[data-test="no_specialist"]')).toHaveAttribute("data-test-state", "not_checked");
    await expect(page.locator("[data-cpc-converted]")).toBeVisible();
    await expect(page.getByLabel("Words to exclude, one a line")).toHaveValue("Pas cher");
    await expect(page.getByLabel("Average price, VAT included")).toHaveValue("60.00");

    // The excluded word drops its keyword; the kept ones say what a click
    // is worth against the sale.
    await listHolds(page, /^Excluded/, [cheap]);
    await listHolds(page, /^Main/, [seed, best]);
    const bestRow = page.locator(`[data-keyword="${best}"]`);
    await expect(bestRow.locator("[data-ratio]")).toHaveText("109.09");
    await expect(bestRow.locator("[data-channel]")).toHaveAttribute("data-channel", "seo_only");
    await testStands(page, "price_per_click", "failed");

    // Dropped from its row, then brought back.
    await bestRow.getByRole("button", { name: "Drop" }).click();
    await expect(page.getByRole("status")).toContainText("Keyword dropped.");
    await listHolds(page, /^Excluded/, [cheap, best]);
    await page.locator(`[data-keyword="${best}"]`).getByRole("button", { name: "Bring back" }).click();
    await expect(page.getByRole("status")).toContainText("Keyword brought back.");
    await listHolds(page, /^Main/, [seed, best]);
    expect(source.calls).toHaveLength(calls);

    // The keywords export with their score, for the owner only.
    const exportUrl = await page.locator("[data-study-export]").getAttribute("href");
    const file = await page.request.get(exportUrl!);
    expect(file.status()).toBe(200);
    expect(file.headers()["content-type"]).toContain("text/csv");
    const lines = (await file.text()).split("\r\n");
    expect(lines[0]).toContain("keyword,outcome,counts_under,searches_a_month,difficulty,cost_per_click_usd");
    expect(lines).toContainEqual(expect.stringContaining(`${seed},main,,12100,38,0.42,`));
    expect(lines).toContainEqual(expect.stringContaining(`${cheap},excluded,,5400,14,0.31,`));
    expect(lines.filter((line) => line !== "")).toHaveLength(6);
    const stranger = await (await browser.newContext()).newPage();
    await registerWithEmail(stranger, uniqueEmail("stranger"));
    expect((await stranger.request.get(exportUrl!)).status()).toBe(404);
    expect((await stranger.goto(studyUrl))?.status()).toBe(404);

    // The list sets the studies side by side.
    const row = page.locator(`[data-study="${name}"]`);
    await expect(async () => {
      await page.goto("/research");
      await expect(row.locator("[data-study-main]")).toHaveText("2", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(row).toHaveAttribute("data-study-verdict", "no_go");
    await expect(row.locator("[data-study-long-tail]")).toHaveText("0");
    await expect(row.locator("[data-study-volume]")).toHaveText("15000");
    await expect(row.locator("[data-study-break-even]")).toHaveText("2.52");
    await expect(row.locator("[data-study-needed]")).toHaveText("194.67");
  } finally {
    await source.stop();
  }
});

/// Reloads the study's page until it lists `count` sites found in the first
/// results: they come from the read model, a moment behind the searches.
async function sitesListed(page: Page, count: number) {
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-site]")).toHaveCount(count, { timeout: 1000 });
  }).toPass({ timeout: 20_000 });
}

/// Sends the keyword of a row of the tree's page where `to` says.
async function move(page: Page, row: string, to: string, said: string) {
  const form = page.locator(row).locator("[data-place]");
  await form.getByRole("combobox").selectOption({ label: to });
  await form.getByRole("button", { name: /^Move/ }).click();
  await expect(page.getByRole("status")).toContainText(said);
}

test("the kept keywords of a study become a category tree", async ({ page, browser }) => {
  const source = new FakeDataforseo();
  await source.start();
  const tag = Date.now().toString(36);
  const seed = `gants moto ${tag}`;
  const cheap = `${seed} pas cher`;
  const best = `meilleur ${seed}`;
  const named = (keyword: string) => keyword.charAt(0).toUpperCase() + keyword.slice(1);
  try {
    await registerWithEmail(page, uniqueEmail("planter"));
    await page.goto("/research/new");
    await page.getByLabel("Name").fill(`Biker gloves ${Date.now()}`);
    await page.getByLabel("Seed keywords, one a line").fill(seed);
    await page.getByRole("button", { name: "Start the study" }).click();
    await expect(page.getByRole("status")).toContainText("Study started.");
    const studyUrl = page.url().split("?")[0];
    await page.getByRole("button", { name: "Fetch the keywords of 1 seed" }).click();
    await seedStands(page, seed, "fetched");
    await keywordsListed(page, 5);
    await listHolds(page, /^Too difficult/, [seed]);
    await page.locator(`[data-keyword="${seed}"]`).getByRole("button", { name: "Bring back" }).click();
    await listHolds(page, /^Main/, [seed, cheap, best]);
    const calls = source.calls.length;

    // The tree starts empty, from the three keywords the study keeps.
    await expect(page.locator("[data-study-categories]")).toHaveAttribute("data-study-categories", "0");
    await page.locator("[data-study-tree]").click();
    const treeUrl = page.url().split("?")[0];
    expect(treeUrl).toBe(`${studyUrl}/tree`);
    await expect(page.locator("[data-tree]")).toHaveAttribute("data-tree", "empty");
    await expect(page.locator("[data-kept]")).toHaveCount(3);
    await expect(page.locator("[data-tree-unpriced]")).toBeVisible();

    // A parent, a child of it, and one parked for later.
    await move(page, `[data-kept="${seed}"]`, "A parent category", "Placed in the tree.");
    await move(page, `[data-kept="${cheap}"]`, `Under ${named(seed)}`, "Placed in the tree.");
    await move(page, `[data-kept="${best}"]`, "Parked for later", "Parked for later.");
    await expect(page.locator("[data-category]")).toHaveCount(2);
    const parent = page.locator(`[data-category="${seed}"]`);
    const child = page.locator(`[data-category="${cheap}"]`);
    await expect(parent).toHaveAttribute("data-category-level", "parent");
    await expect(parent.locator("[data-category-name]")).toHaveText(named(seed));
    await expect(parent).toHaveAttribute("data-category-slug", `gants-moto-${tag}`);
    await expect(parent.locator("[data-volume]")).toHaveText("12100");
    await expect(parent.locator("[data-cpc]")).toHaveText("$0.42");
    await expect(child).toHaveAttribute("data-category-level", "child");
    await expect(child).toHaveAttribute("data-category-slug", `gants-moto-${tag}-pas-cher`);
    await expect(page.locator(`[data-parked="${best}"]`)).toBeVisible();
    await expect(page.locator(`[data-kept="${seed}"]`)).toHaveAttribute("data-kept-stands", "tree");
    await expect(page.locator(`[data-kept="${best}"]`)).toHaveAttribute("data-kept-stands", "parked");
    // A parent stays where it is while it has a child.
    await expect(parent.locator("[data-place]")).toHaveCount(0);
    await expect(parent).toContainText("Move its children first");

    // The niche is named by the words of its seeds until its owner says:
    // a name without one of them is held back.
    await expect(page.locator("[data-tree]")).toHaveAttribute("data-tree", "ready");
    await expect(page.locator("[data-niche-words]")).toHaveAttribute("data-niche-words", "seeds");
    await expect(page.getByLabel("The niche's own words")).toHaveValue(`gants\nmoto\n${tag}`);
    await page.getByLabel("The niche's own words").fill("casque");
    await page.getByRole("button", { name: "Save the words" }).click();
    await expect(page.getByRole("status")).toContainText("Saved: the names are checked again.");
    await expect(page.locator("[data-tree]")).toHaveAttribute("data-tree", "held_back");
    await expect(page.locator("[data-tree-held-back]")).toHaveAttribute("data-tree-held-back", "2");
    await expect(child.locator('[data-category-problem="no_niche_word"]')).toBeVisible();
    await page.getByLabel("The niche's own words").fill("Motos");
    await page.getByRole("button", { name: "Save the words" }).click();
    await expect(page.locator("[data-tree]")).toHaveAttribute("data-tree", "ready");
    await expect(page.locator("[data-niche-words]")).toHaveAttribute("data-niche-words", "said");

    // Once the sale is said, a category whose clicks cost too much against
    // its price is kept out of ads; a price of its own changes that.
    await expect(parent.locator("[data-category-ads]")).toHaveAttribute("data-category-ads", "");
    await page.goto(studyUrl);
    await expect(page.locator("[data-study-categories]")).toHaveAttribute("data-study-categories", "2");
    await page.getByLabel("Average price, VAT included").fill("60");
    await page.getByLabel("Supplier's cost").fill("15");
    await page.getByLabel("Sending a parcel").fill("4");
    await page.getByLabel("At the border").fill("5");
    await page.getByLabel("VAT in the price, in %").fill("20");
    await page.getByLabel("Fixed costs, for each sale").fill("1");
    await page.getByLabel("Payment fee, in % of the price").fill("1.5");
    await page.getByLabel("Payment fee, for each payment").fill("0.25");
    await page.getByRole("button", { name: "Save and score again" }).click();
    await expect(page.locator('[data-sale-line="needed"]')).toContainText("194.67");
    await page.goto(treeUrl);
    await expect(parent.locator("[data-ratio]")).toHaveText("142.85");
    await expect(parent.locator("[data-category-ads]")).toHaveAttribute("data-category-ads", "keep_out");
    await expect(child.locator("[data-ratio]")).toHaveText("193.54");
    await expect(child.locator("[data-category-ads]")).toHaveAttribute("data-category-ads", "keep_out");
    await page.getByLabel(`Average price of ${named(cheap)}, in EUR`).fill("70");
    await child.getByRole("button", { name: /^Save/ }).click();
    await expect(page.getByRole("status")).toContainText("Price saved.");
    await expect(child.locator("[data-category-price]")).toContainText("70.00");
    await expect(child.locator("[data-ratio]")).toHaveText("225.80");
    await expect(child.locator("[data-category-ads]")).toHaveAttribute("data-category-ads", "ads");

    // A parked keyword comes into the tree; a child goes out of it.
    await move(page, `[data-parked="${best}"]`, "A parent category", "Placed in the tree.");
    await expect(page.locator('[data-category-level="parent"]')).toHaveCount(2);
    await expect(page.locator("[data-parked]")).toHaveCount(0);
    await move(page, `[data-category="${cheap}"]`, "Out of the tree", "Taken out.");
    await expect(page.locator("[data-category]")).toHaveCount(2);
    await expect(page.locator(`[data-kept="${cheap}"]`)).toHaveAttribute("data-kept-stands", "");

    // A keyword the study stops keeping is held back where it stands.
    await page.goto(studyUrl);
    await listHolds(page, /^Main/, [seed, cheap, best]);
    await page.locator(`[data-keyword="${best}"]`).getByRole("button", { name: "Drop" }).click();
    await expect(page.getByRole("status")).toContainText("Keyword dropped.");
    await expect(async () => {
      await page.goto(treeUrl);
      await expect(page.locator(`[data-category="${best}"] [data-category-problem="not_kept"]`)).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 20_000 });
    await expect(page.locator("[data-tree]")).toHaveAttribute("data-tree", "held_back");
    await expect(page.locator("[data-kept]")).toHaveCount(2);

    // Nothing was fetched for any of it, and the tree is its owner's only.
    expect(source.calls).toHaveLength(calls);
    const stranger = await (await browser.newContext()).newPage();
    await registerWithEmail(stranger, uniqueEmail("stranger"));
    expect((await stranger.goto(treeUrl))?.status()).toBe(404);
  } finally {
    await source.stop();
  }
});

test("the first results say whether a specialist holds the niche", async ({ page }) => {
  const source = new FakeDataforseo();
  await source.start();
  const tag = Date.now().toString(36);
  const seed = `litière ${tag}`;
  const cheap = `${seed} pas cher`;
  const best = `meilleur ${seed}`;
  const start = async (name: string, seed: string) => {
    await page.goto("/research/new");
    await page.getByLabel("Name").fill(name);
    await page.getByLabel("Seed keywords, one a line").fill(seed);
    await page.getByRole("button", { name: "Start the study" }).click();
    await expect(page.getByRole("status")).toContainText("Study started.");
    await page.getByRole("button", { name: "Fetch the keywords of 1 seed" }).click();
    await seedStands(page, seed, "fetched");
    await keywordsListed(page, 5);
  };
  try {
    await registerWithEmail(page, uniqueEmail("rival"));
    await start(`Litters ${Date.now()}`, seed);

    // Nothing is searched until asked for, and the cost is said before.
    const competition = page.locator("#competition");
    await testStands(page, "main_keywords", "failed");
    await expect(competition).toHaveAttribute("data-competition", "not_checked");
    await expect(competition.locator("[data-site]")).toHaveCount(0);
    await expect(competition.locator("[data-search-cost]")).toContainText("One paid search each: $0.0040 at the most.");
    expect(source.searches).toHaveLength(0);
    await competition.getByRole("button", { name: "Search the first results of 2 main keywords" }).click();
    await expect(page.getByRole("status")).toContainText("The first results are being searched.");

    // The two every search finds, and one of each keyword's own; the ad is
    // not a result.
    await sitesListed(page, 4);
    expect(source.searches).toHaveLength(2);
    expect(source.searches).toContainEqual({ keyword: cheap, locationCode: 2250, languageCode: "fr" });
    const amazon = page.locator('[data-site="amazon.fr"]');
    const zooplus = page.locator('[data-site="zooplus.fr"]');
    await expect(page.locator("[data-site]").first()).toHaveAttribute("data-site", "amazon.fr");
    await expect(amazon.locator("[data-site-keywords]")).toHaveText("2 of 2");
    await expect(amazon.locator("[data-site-places]")).toHaveText("1, 1");
    await expect(zooplus.locator("[data-site-places]")).toHaveText("2, 2");
    await expect(amazon).toHaveAttribute("data-site-decides", "yes");
    const own = page.locator(`[data-site="${sitesFor(best)[2]}"]`);
    await expect(own).toHaveAttribute("data-site-decides", "no");
    await expect(own.locator("[data-site-places]")).toHaveText("3");
    await expect(page.locator('[data-site="ads.example"]')).toHaveCount(0);
    // Both searches were paid, beside the fetch.
    await spentToday(page, 12480 + 4000);

    // Until the sites found for both keywords are marked, nothing is said.
    const test = page.locator('[data-test="no_specialist"]');
    await expect(test).toHaveAttribute("data-test-state", "not_checked");
    await expect(test.locator("[data-test-numbers]")).toContainText("2 sites found for 2 main keywords or more to mark");
    await amazon.getByRole("button", { name: "Generalist" }).click();
    await expect(page).toHaveURL(/#competition$/);
    await expect(amazon).toHaveAttribute("data-site-mark", "generalist");
    await expect(amazon.getByRole("button", { name: "Generalist" })).toHaveCount(0);
    await expect(test).toHaveAttribute("data-test-state", "not_checked");

    // A specialist for both keywords holds the niche; a generalist does not.
    await zooplus.getByRole("button", { name: "Specialist" }).click();
    await expect(test).toHaveAttribute("data-test-state", "failed");
    await expect(test.locator("[data-test-numbers]")).toContainText("1 specialist shop found");
    await expect(competition).toHaveAttribute("data-competition", "failed");
    await zooplus.getByRole("button", { name: "Generalist" }).click();
    await expect(test).toHaveAttribute("data-test-state", "passed");
    await zooplus.getByRole("button", { name: "Specialist" }).click();
    await expect(test).toHaveAttribute("data-test-state", "failed");

    // Every keyword searched: asking again is offered, with its cost.
    await expect(competition.getByRole("button", { name: "Search them all again" })).toBeVisible();
    await expect(competition.locator("[data-search-cost]")).toContainText("costs $0.0040 at the most");

    // The marks are the user's: the next study knows both sites.
    const other = `gamelle ${tag}`;
    await start(`Bowls ${Date.now()}`, other);
    await testStands(page, "main_keywords", "failed");
    await page.locator("#competition").getByRole("button", { name: "Search the first results of 2 main keywords" }).click();
    await sitesListed(page, 4);
    await expect(page.locator('[data-site="amazon.fr"]')).toHaveAttribute("data-site-mark", "generalist");
    await expect(page.locator('[data-site="zooplus.fr"]')).toHaveAttribute("data-site-mark", "specialist");
    await expect(page.locator('[data-test="no_specialist"]')).toHaveAttribute("data-test-state", "failed");
    expect(source.searches).toHaveLength(4);
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
