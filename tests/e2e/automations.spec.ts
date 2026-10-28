import { createHmac } from "node:crypto";
import { createServer, type IncomingHttpHeaders } from "node:http";
import { expect, test, type Locator, type Page } from "@playwright/test";
import { mailTo, registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Automations: the scenarios a shop's team draws under /t/{slug}/automations,
// for members with `automations:read`; those with `automations:manage` draw
// them. The editor works by dragging blocks (palette blocks onto the slots
// between steps, a step by its handle), and the same with buttons only: the
// "+" menu of a slot, then Move / Move here, with or without JavaScript.
// Published, a scenario runs: a contact message handed in through the store
// API makes "Notify the team" mail the owner, and "E-mail the customer" mail
// the visitor through the shop's own server, with a link that stops them.

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

test("a published scenario runs: a contact message tells the team", async ({
  page,
  request,
}) => {
  const owner = uniqueEmail("owner");
  await registerWithEmail(page, owner);
  const { name } = uniqueTenant("Running Shop");
  await createTenant(page, name);

  // The key a storefront hands its contact form's messages in with.
  await page.getByRole("link", { name: "API keys", exact: true }).click();
  await page.getByLabel("Label").fill("Storefront");
  await page.getByLabel(/Fill carts and place orders/).check();
  await page.getByRole("button", { name: "Create key" }).click();
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_/);

  await newScenario(page, "Tell the team");
  await enhanced(page);
  await chooseTrigger(page, "When a contact message arrives");
  await slot(page, "root.0").getByRole("button", { name: "Add a step here" }).click();
  await page
    .locator('[id="add-root.0"]')
    .getByRole("button", { name: "Notify the team", exact: true })
    .click();
  await page.locator("[data-inspector]").getByLabel("Subject").fill("A visitor wrote");
  await page.locator("[data-inspector]").getByLabel("Note").fill("Answer within a day.");
  await page.getByRole("button", { name: "Publish", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Published as version 1");
  await expect(page.locator("[data-scenario-state]")).toHaveAttribute(
    "data-scenario-state",
    "active",
  );

  const sent = await request.post("/store/contact", {
    headers: { Authorization: `Bearer ${token}` },
    form: {
      first_name: "Ada",
      last_name: "Lovelace",
      email: uniqueEmail("visitor"),
      body: "Do you ship to Iceland?",
    },
  });
  expect(sent.status()).toBe(201);

  // The runner mails the owner, who may read the messages.
  const mail = await mailTo(request, owner, `${name}: A visitor wrote`);
  expect(mail.html).toContain("Answer within a day.");
  expect(mail.html).toContain("Tell the team");
  // What the visitor wrote never travels.
  expect(mail.html).not.toContain("Iceland");

  // The run is listed, finished.
  await page.getByRole("link", { name: "Runs", exact: true }).click();
  await expect(async () => {
    await page.reload();
    await expect(page.locator('[data-run="finished"]')).toHaveCount(1, { timeout: 500 });
  }).toPass();
  await page.locator('[data-run="finished"] a').first().click();
  await expect(page.locator("[data-run-status]")).toHaveText("Finished");
  await expect(page.locator("[data-run-log]")).toContainText("Notify the team");

  // Paused, then running again.
  await page.getByRole("link", { name: "Back to the runs" }).click();
  await page.getByRole("link", { name: "Back to the scenario" }).click();
  await page.getByRole("button", { name: "Pause", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Paused");
  await expect(page.locator("[data-scenario-state]")).toHaveAttribute(
    "data-scenario-state",
    "paused",
  );
  await page.getByRole("button", { name: "Resume", exact: true }).click();
  await expect(page.locator("[data-scenario-state]")).toHaveAttribute(
    "data-scenario-state",
    "active",
  );
});

test("an unfinished scenario is not published", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name } = uniqueTenant("Unfinished Shop");
  await createTenant(page, name);
  await newScenario(page, "Half done");
  await page.getByRole("button", { name: "Publish", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("not ready to publish");
  await expect(page.locator("[data-scenario-state]")).toHaveAttribute(
    "data-scenario-state",
    "draft",
  );
});

test("a scenario mails the customer, who stops it from the mail's link", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Mailing Shop");
  await createTenant(page, name);

  // Automated mails go through the shop's own server (the development
  // mailbox stands in) and link to its storefront.
  await page.goto(`/t/${slug}/mail`);
  await page.getByLabel("Server", { exact: true }).fill("localhost");
  await page.getByLabel("Port", { exact: true }).fill("1025");
  await page.getByLabel("Security").selectOption("none");
  await page.getByLabel("Send from address").fill(uniqueEmail("shop"));
  await page.getByRole("button", { name: "Test and save" }).click();
  await expect(page.getByRole("status")).toContainText("Test mail sent");
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Where shoppers find the shop").fill("https://shop.example");
  await page.getByRole("button", { name: "Save address" }).click();
  await expect(page.getByRole("status")).toContainText("Storefront address saved.");
  await page.getByLabel("Label").fill("Storefront");
  await page.getByLabel(/Fill carts and place orders/).check();
  await page.getByRole("button", { name: "Create key" }).click();
  const key = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";

  await newScenario(page, "Thank visitors");
  await enhanced(page);
  await chooseTrigger(page, "When a contact message arrives");
  await slot(page, "root.0").getByRole("button", { name: "Add a step here" }).click();
  await page
    .locator('[id="add-root.0"]')
    .getByRole("button", { name: "E-mail the customer", exact: true })
    .click();
  // It starts from the shop's default automated mail, previewed.
  await expect(page.locator("[data-preview-subject]")).toContainText(name);
  await page.locator("[data-inspector]").getByLabel("Subject").fill("Thanks for writing");
  await page.getByRole("button", { name: "Apply" }).click();
  await expect(page.locator("[data-preview-subject]")).toHaveText("Thanks for writing");
  await page.getByRole("button", { name: "Publish", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Published as version 1");

  const visitor = uniqueEmail("visitor");
  const write = () =>
    request.post("/store/contact", {
      headers: { Authorization: `Bearer ${key}` },
      form: { first_name: "Ada", last_name: "Lovelace", email: visitor, body: "Do you ship abroad?" },
    });
  expect((await write()).status()).toBe(201);
  const mail = await mailTo(request, visitor, "Thanks for writing");
  // The link at its foot (slashes come escaped in the HTML).
  const html = (mail.html ?? "").replace(/&#x2f;/g, "/");
  const token = html.match(/https:\/\/shop\.example\/emails\/opt-out\/([A-Za-z0-9._-]+)/)?.[1];
  expect(token, "the mail links to the opt-out page").toBeTruthy();

  // The storefront's page hands the token back: no more automated mails.
  const stopped = await request.post("/store/automations/opt-out", {
    headers: { Authorization: `Bearer ${key}` },
    form: { token: token! },
  });
  expect(stopped.status()).toBe(204);
  const forged = await request.post("/store/automations/opt-out", {
    headers: { Authorization: `Bearer ${key}` },
    form: { token: `${token}x` },
  });
  expect(forged.status()).toBe(422);

  // The next message's run skips the mail, and says why.
  expect((await write()).status()).toBe(201);
  await page.getByRole("link", { name: "Runs", exact: true }).click();
  await expect(async () => {
    await page.reload();
    await expect(page.locator('[data-run="finished"]')).toHaveCount(2, { timeout: 500 });
  }).toPass();
  await page.locator('[data-run="finished"] a').first().click();
  await expect(page.locator("[data-run-log]")).toContainText(
    "stopped the shop's automated mails",
  );
});

test("a scenario that mails customers needs the shop set up to send them", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name } = uniqueTenant("Serverless Shop");
  await createTenant(page, name);
  await newScenario(page, "Thank buyers");
  await chooseTrigger(page, "When an order is placed");
  await slot(page, "root.0").getByRole("button", { name: "Add a step here" }).click();
  await page
    .locator('[id="add-root.0"]')
    .getByRole("button", { name: "E-mail the customer", exact: true })
    .click();
  await page.getByRole("button", { name: "Publish", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("storefront lives");
});

test("a scenario makes a single-use code and mails it", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Coupon Shop");
  await createTenant(page, name);
  await page.goto(`/t/${slug}/mail`);
  await page.getByLabel("Server", { exact: true }).fill("localhost");
  await page.getByLabel("Port", { exact: true }).fill("1025");
  await page.getByLabel("Security").selectOption("none");
  await page.getByLabel("Send from address").fill(uniqueEmail("shop"));
  await page.getByRole("button", { name: "Test and save" }).click();
  await expect(page.getByRole("status")).toContainText("Test mail sent");
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Where shoppers find the shop").fill("https://shop.example");
  await page.getByRole("button", { name: "Save address" }).click();
  await page.getByLabel("Label").fill("Storefront");
  await page.getByLabel(/Fill carts and place orders/).check();
  await page.getByRole("button", { name: "Create key" }).click();
  const key = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";

  await newScenario(page, "Win them back");
  await chooseTrigger(page, "When a contact message arrives");
  await slot(page, "root.0").getByRole("button", { name: "Add a step here" }).click();
  await page
    .locator('[id="add-root.0"]')
    .getByRole("button", { name: "Create a discount code", exact: true })
    .click();
  await page.locator("[data-inspector]").getByLabel("Code starts with").fill("HELLO");
  await page.getByRole("button", { name: "Apply" }).click();
  await expect(step(page, "s1").locator("[data-step-summary]").first()).toContainText("off, 30 days");
  await slot(page, "root.1").getByRole("button", { name: "Add a step here" }).click();
  await page
    .locator('[id="add-root.1"]')
    .getByRole("button", { name: "E-mail the customer", exact: true })
    .click();
  await page.locator("[data-inspector]").getByLabel("Subject").fill("A code for you");
  await page.getByRole("button", { name: "Apply" }).click();
  await page.getByRole("button", { name: "Publish", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Published as version 1");

  const visitor = uniqueEmail("visitor");
  const sent = await request.post("/store/contact", {
    headers: { Authorization: `Bearer ${key}` },
    form: { first_name: "Ada", last_name: "Lovelace", email: visitor, body: "Do you have a discount?" },
  });
  expect(sent.status()).toBe(201);
  // The default mail quotes the code the step before it made.
  const mail = await mailTo(request, visitor, "A code for you");
  const code = mail.html?.match(/HELLO-[A-Z0-9]{8}/)?.[0];
  expect(code, "the mail quotes the code").toBeTruthy();
  // It is one of the shop's discount codes, usable once.
  await page.getByRole("link", { name: "Discounts", exact: true }).click();
  await expect(async () => {
    await page.reload();
    await expect(page.getByText(code!)).toBeVisible({ timeout: 500 });
  }).toPass();
});

test("a scenario calls a webhook, signed with the step's secret", async ({ page, request }) => {
  // The merchant's own app: a server on this machine (the e2e config lets a
  // shop call one) that keeps what it is sent.
  const received: { headers: IncomingHttpHeaders; body: string }[] = [];
  const server = createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      received.push({ headers: req.headers, body });
      res.writeHead(200);
      res.end("ok");
    });
  });
  await new Promise<void>((resolve) => server.listen(12117, "127.0.0.1", resolve));
  try {
    await registerWithEmail(page, uniqueEmail("owner"));
    const { name, slug } = uniqueTenant("Hook Shop");
    await createTenant(page, name);
    await page.goto(`/t/${slug}/api-keys`);
    await page.getByLabel("Label").fill("Storefront");
    await page.getByLabel(/Fill carts and place orders/).check();
    await page.getByRole("button", { name: "Create key" }).click();
    const key = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";

    await newScenario(page, "Tell my CRM");
    await chooseTrigger(page, "When a contact message arrives");
    await slot(page, "root.0").getByRole("button", { name: "Add a step here" }).click();
    await page
      .locator('[id="add-root.0"]')
      .getByRole("button", { name: "Call a webhook", exact: true })
      .click();
    await page.locator("[data-inspector]").getByLabel("Address").fill("http://127.0.0.1:12117/hook");
    await page.getByRole("button", { name: "Apply" }).click();
    // Sealed once applied: only its host shows, and the secret to check with.
    await expect(step(page, "s1").locator("[data-step-summary]").first()).toHaveText(
      "Calls 127.0.0.1",
    );
    await expect(page.locator("[data-scenario-editor]")).not.toContainText("12117/hook");
    const secret = await page.locator("[data-webhook-secret]").inputValue();
    expect(secret).toMatch(/^whsec_[0-9a-f]{64}$/);
    await page.getByRole("button", { name: "Publish", exact: true }).click();
    await expect(page.getByRole("status")).toContainText("Published as version 1");

    const sent = await request.post("/store/contact", {
      headers: { Authorization: `Bearer ${key}` },
      form: { first_name: "Ada", last_name: "Lovelace", email: uniqueEmail("visitor"), body: "Please call me back." },
    });
    expect(sent.status()).toBe(201);

    await expect.poll(() => received.length, { timeout: 15_000 }).toBe(1);
    const call = received[0];
    const payload = JSON.parse(call.body);
    expect(payload.type).toBe("message.received");
    expect(payload.shop).toBe(slug);
    expect(payload.subject.kind).toBe("message");
    expect(payload.scenario.name).toBe("Tell my CRM");
    // Ids only: nothing the visitor wrote.
    expect(call.body).not.toContain("call me back");
    expect(call.headers["idempotency-key"]).toBe(payload.id);
    const signature = String(call.headers["timada-signature"]);
    const t = signature.match(/t=(\d+)/)?.[1];
    const v1 = signature.match(/v1=([0-9a-f]+)/)?.[1];
    expect(createHmac("sha256", secret).update(`${t}.${call.body}`).digest("hex")).toBe(v1);
  } finally {
    server.close();
  }
});
