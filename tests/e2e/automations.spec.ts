import { expect, test, type Locator, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Automations: the scenarios a shop's team draws under /t/{slug}/automations,
// for members with `automations:read`; those with `automations:manage` draw
// them. The editor works by dragging blocks (palette blocks onto the slots
// between steps, a step by its handle), and the same with buttons only: the
// "+" menu of a slot, then Move / Move here, with or without JavaScript.

/// A new scenario, its editor open.
async function newScenario(page: Page, name: string) {
  await page.getByRole("link", { name: "Automations", exact: true }).click();
  await page.getByRole("link", { name: "New scenario", exact: true }).click();
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Create scenario" }).click();
  await expect(page).toHaveURL(/\/automations\/[0-9A-Z]{26}$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(name);
}

/// The editor once its script has wired the dragging.
async function enhanced(page: Page) {
  await expect(page.locator("[data-scenario-editor][data-enhanced]")).toHaveCount(1);
}

/// Drags `source` onto `target` with the mouse, in steps, as a hand would.
/// The slots grow once a drag starts: the target is measured after.
async function drag(page: Page, source: Locator, target: Locator) {
  const from = await source.boundingBox();
  if (!from) throw new Error("the source is not on screen");
  const x = from.x + from.width / 2;
  const y = from.y + from.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 12, y + 12, { steps: 4 });
  await target.scrollIntoViewIfNeeded();
  const to = await target.boundingBox();
  if (!to) throw new Error("the target is not on screen");
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
  await page.mouse.up();
}

const slot = (page: Page, at: string) =>
  page.locator(`[data-drop-slot="${at}"]`);
const step = (page: Page, id: string) => page.locator(`[data-step="${id}"]`);

/// Picks the trigger in the inspector.
async function chooseTrigger(page: Page, label: string) {
  await page.locator("[data-trigger] button").click();
  await page
    .locator("[data-inspector]")
    .getByRole("button", { name: new RegExp(label) })
    .click();
  await expect(page.locator("[data-trigger]")).toContainText(label);
}

test("an owner sees the automations section, empty at first", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Robot Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Automations", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/automations$`));
  await expect(page.locator("[data-scenario-count]")).toHaveText("0 scenario(s)");
  await expect(page.getByText("No scenario here.")).toBeVisible();
});

test("a scenario is drawn by dragging blocks, then saved", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name } = uniqueTenant("Drag Shop");
  await createTenant(page, name);
  await newScenario(page, "Big orders");
  await enhanced(page);

  await chooseTrigger(page, "When an order is placed");

  // A condition dragged from the palette under the trigger opens its
  // settings.
  await drag(page, page.locator('[data-block-kind="condition"]'), slot(page, "root.0"));
  await expect(step(page, "s1")).toHaveAttribute("data-step-kind", "condition");
  await expect(page.locator("[data-scenario-editor]")).toHaveAttribute("data-dirty", "");
  await page.locator("[data-inspector]").getByLabel("Value").fill("50");
  await page.getByRole("button", { name: "Apply" }).click();
  await expect(step(page, "s1").locator("[data-step-summary]").first()).toHaveText(
    "If Order total ≥ 50.00",
  );

  // A notice dragged into the Yes lane.
  await drag(page, page.locator('[data-block-kind="notify"]'), slot(page, "s1.0.0"));
  await expect(page.locator('[data-branch="s1.0"] [data-step="s2"]')).toHaveCount(1);
  await page.locator("[data-inspector]").getByLabel("Subject").fill("A big order");
  await page.getByRole("button", { name: "Apply" }).click();
  await expect(page.locator("[data-to-set-up]")).toHaveAttribute("data-to-set-up", "0");

  // The condition ends the lane under the trigger: each of its lanes ends
  // on its own, and nothing goes under it.
  await expect(page.locator('[data-branch="s1.0"] [data-lane-end]')).toHaveCount(1);
  await expect(page.locator('[data-branch="s1.1"] [data-lane-end]')).toHaveCount(1);
  await expect(page.locator("[data-lane-end]")).toHaveCount(2);
  await expect(slot(page, "root.1")).toHaveCount(0);

  // Moved by its handle into the No lane.
  await drag(page, step(page, "s2").locator("[data-drag-handle]"), slot(page, "s1.1.0"));
  await expect(page.locator('[data-branch="s1.0"] [data-step]')).toHaveCount(0);
  await expect(page.locator('[data-branch="s1.1"] [data-step="s2"]')).toHaveCount(1);

  await page.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByRole("status")).toContainText("Draft saved.");
  // Read back from the store: the flow as drawn.
  await page.reload();
  await expect(page.locator("[data-trigger]")).toContainText("When an order is placed");
  await expect(step(page, "s1")).toHaveAttribute("data-step-kind", "condition");
  await expect(step(page, "s2")).toHaveAttribute("data-step-kind", "notify");
  await expect(page.locator("[data-unsaved]")).toHaveCount(0);

  // The list is a read model, a moment behind the save: reload until it
  // shows it.
  await page.getByRole("link", { name: "Back to Automations" }).click();
  const row = page.locator("tr", { hasText: "Big orders" });
  await expect(async () => {
    await page.reload();
    await expect(row).toContainText("When an order is placed", { timeout: 500 });
  }).toPass();
  await expect(row).toContainText("Draft");
});

test("a scenario is drawn with buttons only, without JavaScript", async ({ browser }) => {
  const signedIn = await browser.newContext();
  const page = await signedIn.newPage();
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name } = uniqueTenant("Keyboard Shop");
  await createTenant(page, name);
  await newScenario(page, "Welcome");
  const editor = page.url();

  const plain = await browser.newContext({
    javaScriptEnabled: false,
    storageState: await signedIn.storageState(),
  });
  const noJs = await plain.newPage();
  await noJs.goto(editor);

  // The "+" menu of a slot adds a block there.
  await slot(noJs, "root.0").getByRole("button", { name: "Add a step here" }).click();
  await noJs.locator('[id="add-root.0"]').getByRole("button", { name: "Wait", exact: true }).click();
  await expect(step(noJs, "s1")).toHaveAttribute("data-step-kind", "wait");
  await slot(noJs, "root.1").getByRole("button", { name: "Add a step here" }).click();
  await noJs
    .locator('[id="add-root.1"]')
    .getByRole("button", { name: "Notify the team", exact: true })
    .click();
  await expect(step(noJs, "s2")).toHaveAttribute("data-step-kind", "notify");

  // Move, then Move here: the notice goes above the wait.
  await noJs.getByRole("button", { name: "Move Notify the team", exact: true }).click();
  await expect(noJs.locator("[data-moving]")).toBeVisible();
  await slot(noJs, "root.0").getByRole("button", { name: "Move here" }).click();
  await expect(noJs.locator("[data-step]").first()).toHaveAttribute("data-step", "s2");
  // Up and down need no drag either.
  await noJs.getByRole("button", { name: "Move Notify the team down", exact: true }).click();
  await expect(noJs.locator("[data-step]").first()).toHaveAttribute("data-step", "s1");
  await noJs.getByRole("button", { name: "Remove Wait", exact: true }).click();
  await expect(noJs.locator("[data-step]")).toHaveCount(1);

  await noJs.getByRole("button", { name: "Save draft" }).click();
  await expect(noJs.getByRole("status")).toContainText("Draft saved.");
  await expect(step(noJs, "s2")).toHaveCount(1);

  await plain.close();
  await signedIn.close();
});

test("a copy saved over by someone else is refused", async ({ browser }) => {
  const context = await browser.newContext();
  const first = await context.newPage();
  await registerWithEmail(first, uniqueEmail("owner"));
  const { name } = uniqueTenant("Race Shop");
  await createTenant(first, name);
  await newScenario(first, "Shared");
  const second = await context.newPage();
  await second.goto(first.url());

  await first.getByLabel("Name").fill("Shared, first");
  await first.getByRole("button", { name: "Save draft" }).click();
  await expect(first.getByRole("status")).toContainText("Draft saved.");

  await second.getByLabel("Name").fill("Shared, second");
  await second.getByRole("button", { name: "Save draft" }).click();
  await expect(second.getByRole("alert")).toContainText("Someone saved this scenario");
  // The copy is still on the page, to keep what was typed.
  await expect(second.getByLabel("Name")).toHaveValue("Shared, second");

  await context.close();
});

test("the automations need automations:read, drawing automations:manage", async ({
  browser,
  request,
}) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Robots");
  await createTenant(owner, name);
  await newScenario(owner, "Read me");
  const scenario = new URL(owner.url()).pathname;

  await owner.getByRole("link", { name: "Roles", exact: true }).click();
  const define = owner.locator("form", {
    has: owner.getByRole("button", { name: "Define role" }),
  });
  await define.getByLabel("Role name").fill("viewer");
  await define.getByLabel("Label").fill("Viewer");
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role defined.");
  await define.getByLabel("Role name").fill("reader");
  await define.getByLabel("Label").fill("Reader");
  await define.getByLabel(/See the automation scenarios/).check();
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role defined.");

  const viewerEmail = uniqueEmail("viewer");
  const readerEmail = uniqueEmail("reader");
  await owner.getByRole("link", { name: "Members", exact: true }).click();
  await invite(owner, viewerEmail, "Viewer");
  await invite(owner, readerEmail, "Reader");
  // Read before registering: the code mailed then is the newest message.
  const links = {
    [viewerEmail]: await inviteLink(request, viewerEmail),
    [readerEmail]: await inviteLink(request, readerEmail),
  };
  const join = async (email: string) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await registerWithEmail(page, email);
    await page.goto(links[email]);
    await page.getByRole("button", { name: "Accept the invitation" }).click();
    await expect(page).toHaveURL(new RegExp(`/t/${slug}`));
    return { context, page };
  };

  // Without automations:read: no section, and the pages are refused.
  const viewer = await join(viewerEmail);
  await expect(
    viewer.page.getByRole("link", { name: "Automations", exact: true }),
  ).toHaveCount(0);
  expect((await viewer.page.goto(`/t/${slug}/automations`))?.status()).toBe(403);
  expect((await viewer.page.goto(scenario))?.status()).toBe(403);

  // With it: the list and the flow, read-only.
  const reader = await join(readerEmail);
  await reader.page.getByRole("link", { name: "Automations", exact: true }).click();
  await expect(reader.page.getByRole("link", { name: "New scenario" })).toHaveCount(0);
  await reader.page.getByRole("link", { name: "Read me" }).click();
  await expect(reader.page.locator("[data-scenario-flow]")).toBeVisible();
  await expect(reader.page.locator("[data-block-kind]")).toHaveCount(0);
  await expect(reader.page.getByRole("button", { name: "Save draft" })).toHaveCount(0);
  const refused = await reader.page.request.post(scenario, {
    form: { op: "save", definition: "", base: "1", name: "Mine now" },
    maxRedirects: 0,
  });
  expect(refused.status()).toBe(403);
  expect(
    (await reader.page.goto(`/t/${slug}/automations/new`))?.status(),
  ).toBe(403);

  await ownerContext.close();
  await viewer.context.close();
  await reader.context.close();
});
