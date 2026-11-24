import { expect, test, type Page } from "@playwright/test";
import { mailTo, registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Proposals: a change to a product is prepared under /t/{slug}/proposals/new,
// waits on /t/{slug}/proposals, and is applied only once a member who may
// approve says yes. One whose product changed meanwhile ends out of date.

/// Creates a product from the products page and returns its address.
async function createProduct(page: Page, slug: string, name: string): Promise<string> {
  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  return page.url().split("?")[0];
}

/// From a product's page: suggests `name` for it, with `why`.
async function suggestName(page: Page, name: string, why: string) {
  await page.getByRole("link", { name: "Suggest a change" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Suggest a change to");
  await page.getByLabel("Name").fill(name);
  await page.getByLabel("Why (optional)").fill(why);
  await page.getByRole("button", { name: "Send for approval" }).click();
  await expect(page.getByRole("status")).toContainText("Prepared: it waits for an answer.");
}

/// The list is a read model, a moment behind the write: reloads until it
/// shows `count` rows.
async function waitForRows(page: Page, count: number) {
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-proposal]")).toHaveCount(count, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
}

test("a prepared change waits, is edited, approved and applied", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Proposal Shop");
  await createTenant(page, name);
  const productUrl = await createProduct(page, slug, "Blue Mug");

  // A change that changes nothing is refused, and comes back as typed.
  await page.getByRole("link", { name: "Suggest a change" }).click();
  await page.getByLabel("Why (optional)").fill("No reason.");
  await page.getByRole("button", { name: "Send for approval" }).click();
  await expect(page.getByRole("alert").first()).toContainText("Nothing would change");
  await expect(page.getByLabel("Why (optional)")).toHaveValue("No reason.");

  // A name and a title for search engines, with why.
  await page.getByLabel("Name").fill("Blue Mug XL");
  await page.getByLabel("Search engines' title").fill("Blue mug, extra large");
  await page.getByLabel("Why (optional)").fill("Clearer for search.");
  await page.getByRole("button", { name: "Send for approval" }).click();
  await expect(page.getByRole("status")).toContainText("Prepared: it waits for an answer.");
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/proposals\\?`));

  // It waits, under what prepared it, with the value before and after.
  await waitForRows(page, 1);
  const group = page.locator("[data-proposal-group=person]");
  await expect(group.getByRole("heading")).toContainText("Written by a member");
  await expect(group).toHaveAttribute("data-proposal-waiting", "1");
  const row = group.locator("[data-proposal]");
  await expect(row).toHaveAttribute("data-proposal-status", "waiting");
  const nameChange = row.locator("[data-proposal-change=name]");
  await expect(nameChange.locator("[data-proposal-before]")).toHaveText("Blue Mug");
  await expect(nameChange.locator("[data-proposal-after]")).toHaveText("Blue Mug XL");
  await expect(row).toContainText("Clearer for search.");

  // Nothing changed yet.
  await page.goto(productUrl);
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Blue Mug");
  await expect(page.getByRole("heading", { level: 1 })).not.toContainText("XL");

  // The owner reads it, rewords the title, and approves.
  await page.goto(`/t/${slug}/proposals`);
  await page.locator("[data-proposal]").getByRole("link", { name: "Blue Mug" }).click();
  await expect(page.locator("[data-proposal-reason]")).toHaveText("Clearer for search.");
  await expect(page.getByLabel("Name")).toHaveValue("Blue Mug XL");
  await page.getByLabel("Search engines' title").fill("Blue mug XL");
  await page.getByRole("button", { name: "Approve" }).click();
  await expect(page.getByRole("status")).toContainText("Approved: the change is being made.");
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-proposal-status]")).toHaveAttribute("data-proposal-status", "applied", {
      timeout: 1000,
    });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("[data-proposal-answered-by]")).toContainText("edited before approving");
  // Answered: no form left.
  await expect(page.getByRole("button", { name: "Approve" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Refuse" })).toHaveCount(0);

  // The product holds the approved values.
  await page.goto(productUrl);
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Blue Mug XL");
  await expect(page.getByText("blue-mug-xl", { exact: true })).toBeVisible();

  // The answered tab keeps it; nothing waits.
  await page.goto(`/t/${slug}/proposals`);
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-proposals-empty]")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.locator("[data-page-tabs]").getByRole("link", { name: "Answered" }).click();
  const answered = page.locator("[data-proposal]");
  // The list trails the proposal's own page by a moment.
  await expect(async () => {
    await page.reload();
    await expect(answered).toHaveAttribute("data-proposal-status", "applied", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(answered.locator("[data-proposal-change=meta_title] [data-proposal-after]")).toHaveText(
    "Blue mug XL",
  );
});

test("a change to a product edited meanwhile ends out of date", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Stale Shop");
  await createTenant(page, name);
  const productUrl = await createProduct(page, slug, "Tea Pot");
  await suggestName(page, "Tea Pot 1 L", "Says the size.");
  await page.goto(productUrl);
  await suggestName(page, "Teapot", "One word.");

  // The product is renamed by hand before anyone answers.
  await page.goto(productUrl);
  await page.getByLabel("Name", { exact: true }).fill("Cast Iron Tea Pot");
  await page.getByRole("button", { name: "Rename" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Cast Iron Tea Pot");

  // The proposal's page says so before the member answers.
  await page.goto(`/t/${slug}/proposals`);
  await waitForRows(page, 2);
  await page.locator("[data-proposal]").first().getByRole("link", { name: "Tea Pot" }).click();
  await expect(page.locator("[data-proposal-stale]")).toContainText("Name was changed since this was prepared");

  // Answering every row shown: neither is applied over the newer name.
  await page.goto(`/t/${slug}/proposals`);
  await page.getByRole("button", { name: "Approve all shown" }).click();
  await expect(page.getByRole("status")).toContainText("2 out of date");
  await page.goto(productUrl);
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Cast Iron Tea Pot");

  await page.goto(`/t/${slug}/proposals?view=history`);
  await waitForRows(page, 2);
  const ended = page.locator("[data-proposal]").first();
  await expect(ended).toHaveAttribute("data-proposal-status", "out_of_date");
  await expect(ended.locator("[data-proposal-outcome]")).toContainText(
    "Name was changed meanwhile, to: Cast Iron Tea Pot",
  );
});

test("a member who only prepares cannot approve; the owner is told and refuses", async ({ browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  const ownerEmail = uniqueEmail("owner");
  await registerWithEmail(owner, ownerEmail);
  const { name, slug } = uniqueTenant("Writers Shop");
  await createTenant(owner, name);
  const productUrl = await createProduct(owner, slug, "Wool Scarf");

  // A role that sees the catalog and prepares changes, nothing more.
  await owner.goto(`/t/${slug}/roles`);
  const define = owner.locator("form", { has: owner.getByRole("button", { name: "Define role" }) });
  await define.getByLabel("Role name").fill("writer");
  await define.getByLabel("Label").fill("Writer");
  await define.getByLabel(/See the products and categories/).check();
  await define.getByLabel(/Prepare a change to the shop/).check();
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role defined.");

  const writerEmail = uniqueEmail("writer");
  await owner.goto(`/t/${slug}/members`);
  await invite(owner, writerEmail, "Writer");
  const link = await inviteLink(owner.request, writerEmail);
  const writerContext = await browser.newContext();
  const writer = await writerContext.newPage();
  await registerWithEmail(writer, writerEmail);
  await writer.goto(link);
  await writer.getByRole("button", { name: "Accept the invitation" }).click();
  await expect(writer.getByRole("status")).toContainText(`You joined ${name}.`);

  // The writer suggests a name; the product page gives them no form.
  await writer.goto(productUrl);
  await expect(writer.getByRole("button", { name: "Rename" })).toHaveCount(0);
  await suggestName(writer, "Merino Wool Scarf", "Says what it is made of.");
  await waitForRows(writer, 1);
  await expect(writer.getByRole("button", { name: "Approve the selected" })).toHaveCount(0);
  await writer.locator("[data-proposal]").getByRole("link", { name: "Wool Scarf" }).click();
  await expect(writer.getByRole("button", { name: "Approve" })).toHaveCount(0);
  await expect(writer.getByRole("button", { name: "Refuse" })).toHaveCount(0);
  const proposalUrl = writer.url().split("?")[0];
  // Answering by hand is refused too.
  const forced = await writer.request.post(`${proposalUrl}/answer`, { form: { verdict: "approve" } });
  expect(forced.status()).toBe(403);

  // The owner is mailed that changes wait, with the list's address.
  const mail = await mailTo(owner.request, ownerEmail, `Changes wait for your approval on ${name}`);
  expect(mail.html ?? "").toContain(`/t/${slug}/proposals`);

  // The owner refuses, with why; the writer reads it.
  await owner.goto(proposalUrl);
  await owner.getByLabel("Why (optional)").fill("Not our tone.");
  await owner.getByRole("button", { name: "Refuse" }).click();
  await expect(owner.getByRole("status")).toContainText("Refused.");
  await expect(owner.locator("[data-proposal-status]")).toHaveAttribute("data-proposal-status", "refused");
  await writer.goto(proposalUrl);
  await expect(writer.locator("[data-proposal-outcome]")).toHaveText("Not our tone.");
  await owner.goto(productUrl);
  await expect(owner.getByRole("heading", { level: 1 })).toContainText("Wool Scarf");
  await expect(owner.getByRole("heading", { level: 1 })).not.toContainText("Merino");

  await ownerContext.close();
  await writerContext.close();
});
