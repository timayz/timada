import { expect, test, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { productTab } from "./forms";
import { createTenant, uniqueTenant } from "./tenants";
import { FakeWriter, textsFor } from "./writer";

// Text drafts, end to end against a stand-in for the model (writer.ts): a
// member asks for a product's texts from its page or for several from the
// products list, the draft is written off the page and waits as a proposal,
// and nothing changes until the proposal is approved. A draft that states
// what the shop's data does not is marked; one the model refuses or cannot
// answer says why.

/// Creates a product from the products page and returns its address.
async function createProduct(page: Page, slug: string, name: string): Promise<string> {
  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  return page.url().split("?")[0];
}

/// From a product's page: asks for its texts, written for `keyword`.
async function askDraft(page: Page, keyword: string) {
  await page.getByLabel("Search to write for (optional)").fill(keyword);
  await page.getByRole("button", { name: "Draft the texts" }).click();
  await expect(page.getByRole("status")).toContainText("Asked: the draft will wait under Proposals");
}

/// Reloads the writer's page until `count` drafts stand at `status`.
async function draftsStand(page: Page, slug: string, status: string, count: number) {
  await page.goto(`/t/${slug}/writer`);
  await expect(async () => {
    await page.reload();
    await expect(page.locator(`[data-draft-status=${status}]`)).toHaveCount(count, { timeout: 1000 });
  }).toPass({ timeout: 20_000 });
}

test("a product's texts are drafted, wait as a proposal and are applied once approved", async ({ page }) => {
  const writer = new FakeWriter();
  await writer.start();
  const keyword = `gants moto été ${Date.now().toString(36)}`;
  const texts = textsFor(keyword, true);
  try {
    await registerWithEmail(page, uniqueEmail("owner"));
    const { name, slug } = uniqueTenant("Writer Shop");
    await createTenant(page, name);

    // The shop says how it wants to be written for.
    await page.getByRole("link", { name: "Text drafts", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${slug}/writer$`));
    await expect(page.getByText("No draft was asked for yet.")).toBeVisible();
    await page.getByLabel("Tone").fill("Direct, on vouvoie.");
    await page.getByLabel("Never claim").fill("Jamais « le meilleur ».");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText("Saved.");
    await expect(page.getByLabel("Tone")).toHaveValue("Direct, on vouvoie.");

    const productUrl = await createProduct(page, slug, "Gants Homme Respirant");
    await askDraft(page, keyword);

    // Written off the page: the draft ends as a proposal, and is paid.
    await draftsStand(page, slug, "drafted", 1);
    const draft = page.locator("[data-draft]");
    await expect(draft).toContainText("Gants Homme Respirant");
    await expect(draft).toContainText(keyword);
    await expect(draft).toContainText("0.02 USD");
    await expect(page.locator("[data-writer-calls]")).toHaveAttribute("data-writer-calls", "1");
    await expect(page.locator("[data-writer-spent]")).toHaveAttribute("data-writer-spent", "24000");

    // The model was given the search, the product and the shop's brief.
    expect(writer.calls).toHaveLength(1);
    expect(writer.calls[0].model).toBe("claude-opus-5-5");
    expect(writer.calls[0].product).toBe(true);
    expect(writer.calls[0].said).toContain("Name: Gants Homme Respirant");
    expect(writer.calls[0].said).toContain("The shop's tone:\nDirect, on vouvoie.");
    expect(writer.calls[0].said).toContain("Never claim or say:\nJamais « le meilleur ».");

    // Nothing was published.
    await page.goto(productUrl);
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Gants Homme Respirant");

    // The proposal holds the four texts, under the writer's name.
    await draftsStand(page, slug, "drafted", 1);
    await page.getByRole("link", { name: "Read the proposal" }).click();
    await expect(page.locator("[data-proposal-reason]")).toHaveText(
      `Drafted by the writer for the search “${keyword}”.`,
    );
    await expect(page.getByLabel("Name")).toHaveValue(texts.name);
    await expect(page.getByLabel("Description", { exact: true })).toHaveValue(texts.description);
    await expect(page.getByLabel("Search engines' title")).toHaveValue(texts.meta_title);
    await expect(page.getByLabel("Search engines' description")).toHaveValue(texts.meta_description);

    // The member corrects the description and approves.
    await page.getByLabel("Description", { exact: true }).fill("Des gants légers pour rouler l'été.");
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("status")).toContainText("Approved: the change is being made.");
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-proposal-status]")).toHaveAttribute("data-proposal-status", "applied", {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });

    // The product holds them, and says a machine wrote its name and its
    // description, as the shopping feed has to.
    await page.goto(productUrl);
    await expect(page.getByRole("heading", { level: 1 })).toContainText(texts.name);
    await expect(page.getByLabel("Description", { exact: true })).toHaveValue("Des gants légers pour rouler l'été.");
    await productTab(page, "Shopping feed");
    await expect(page.getByLabel("AI wrote the name")).toBeChecked();
    await expect(page.getByLabel("AI wrote the description")).toBeChecked();
  } finally {
    await writer.stop();
  }
});

test("a draft that states what the data does not is marked; a failed one says why", async ({ page }) => {
  const writer = new FakeWriter();
  await writer.start();
  const tag = Date.now().toString(36);
  try {
    await registerWithEmail(page, uniqueEmail("owner"));
    const { name, slug } = uniqueTenant("Careful Shop");
    await createTenant(page, name);

    // The stand-in states a length and a material the product never gave.
    const bagUrl = await createProduct(page, slug, "Sac Banane");
    await askDraft(page, `sac banane invente ${tag}`);
    await draftsStand(page, slug, "drafted", 1);
    await expect(page.locator("[data-draft-to-check]")).toHaveText("To check: 30 cm, cuir");
    await page.getByRole("link", { name: "Read the proposal" }).click();
    await expect(page.locator("[data-proposal-reason]")).toContainText(
      "To check, not found in the shop's data: 30 cm, cuir.",
    );

    // The model declines one, and does not answer another: each says why,
    // and neither leaves a proposal.
    const capUrl = await createProduct(page, slug, "Casquette");
    await askDraft(page, `refused ${tag}`);
    await draftsStand(page, slug, "failed", 1);
    await expect(page.locator("[data-draft-status=failed] [data-draft-reason]")).toHaveText(
      "The writer refused: the model declined to write this",
    );
    const hatUrl = await createProduct(page, slug, "Bonnet");
    await askDraft(page, `unavailable ${tag}`);
    await draftsStand(page, slug, "failed", 2);
    await expect(page.locator("[data-draft-status=failed] [data-draft-reason]").first()).toHaveText(
      "The writer did not answer: Overloaded",
    );
    // The declined call was paid for, like the first; the one that failed
    // was not.
    await expect(page.locator("[data-writer-calls]")).toHaveAttribute("data-writer-calls", "2");
    await page.goto(`/t/${slug}/proposals`);
    await expect(page.locator("[data-proposal]")).toHaveCount(1);

    // Several at once, from the products list: the two that failed are
    // drafted again, each for its own name.
    // The list is a read model, a moment behind; drafts are not its default.
    await expect(async () => {
      await page.goto(`/t/${slug}/products?status=all`);
      await expect(page.getByLabel("Select Bonnet")).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await page.getByLabel("Select Casquette").check();
    await page.getByLabel("Select Bonnet").check();
    await page.getByRole("button", { name: "Draft the ticked products' texts" }).click();
    await expect(page.getByRole("status")).toContainText("2 drafts asked for");
    await draftsStand(page, slug, "drafted", 3);
    expect(writer.calls.map((call) => call.keyword).slice(-2).sort()).toEqual(["Bonnet", "Casquette"]);
    await page.goto(`/t/${slug}/proposals`);
    await expect(async () => {
      await page.reload();
      await expect(page.locator("[data-proposal-group=writer]")).toHaveAttribute("data-proposal-waiting", "3", {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });
    await expect(page.locator("[data-proposal-group=writer]").getByRole("heading")).toContainText("Writer");
    expect([bagUrl, capUrl, hatUrl]).toHaveLength(3);
  } finally {
    await writer.stop();
  }
});
