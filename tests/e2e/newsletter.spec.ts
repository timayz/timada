import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Newsletter: who asked for a shop's news is listed under
// /t/{slug}/newsletter for members with `newsletter:read`; those with
// `newsletter:manage` unsubscribe and erase. A shopper signing up from the
// Chatounette example, confirming from the mail and leaving again is
// covered in `chatounette.spec.ts`.

test("an owner sees the newsletter section, empty at first", async ({
  page,
}) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("News Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Newsletter", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/newsletter$`));
  await expect(page.locator("[data-subscriber-count]")).toHaveText(
    "0 subscriber(s)",
  );
  await expect(page.getByText("No subscriber here.")).toBeVisible();
  await expect(
    page.getByText("0 subscriber(s) get the shop's news."),
  ).toBeVisible();
});

test("an owner sets where the shop lives", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name } = uniqueTenant("Zoned Shop");
  await createTenant(page, name);

  const form = page.locator("[data-shop-time-zone]");
  await expect(form.getByLabel("Time zone")).toHaveValue("UTC");
  await form.getByLabel("Time zone").selectOption("Europe/Paris");
  await form.getByRole("button", { name: "Save the time zone" }).click();
  await expect(page.getByText("Time zone saved.")).toBeVisible();
  await expect(form.getByLabel("Time zone")).toHaveValue("Europe/Paris");
});

test("the newsletter page needs newsletter:read, its actions newsletter:manage", async ({
  browser,
  request,
}) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked News");
  await createTenant(owner, name);

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
  await define
    .getByLabel(/See who subscribed to the shop's newsletter/)
    .check();
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

  // Without newsletter:read: no section, and the page is refused.
  const viewer = await join(viewerEmail);
  await expect(
    viewer.page.getByRole("link", { name: "Newsletter", exact: true }),
  ).toHaveCount(0);
  expect((await viewer.page.goto(`/t/${slug}/newsletter`))?.status()).toBe(403);

  // With it: the list, without the actions.
  const reader = await join(readerEmail);
  await reader.page
    .getByRole("link", { name: "Newsletter", exact: true })
    .click();
  await expect(reader.page).toHaveURL(new RegExp(`/t/${slug}/newsletter$`));
  await expect(
    reader.page.getByRole("columnheader", { name: "Actions" }),
  ).toHaveCount(0);
  for (const action of ["unsubscribe", "erase"]) {
    const refused = await reader.page.request.post(
      `/t/${slug}/newsletter/${action}`,
      { form: { subscriber_id: "unknown" }, maxRedirects: 0 },
    );
    expect(refused.status()).toBe(403);
  }
  // The campaigns are read, not written.
  await reader.page.getByRole("link", { name: "Campaigns" }).click();
  await expect(reader.page).toHaveURL(
    new RegExp(`/t/${slug}/newsletter/campaigns$`),
  );
  await expect(
    reader.page.getByRole("link", { name: "New campaign" }),
  ).toHaveCount(0);
  expect(
    (await reader.page.goto(`/t/${slug}/newsletter/campaigns/new`))?.status(),
  ).toBe(403);
  expect(
    (await viewer.page.goto(`/t/${slug}/newsletter/campaigns`))?.status(),
  ).toBe(403);
  // The owner may, and an unknown subscriber is told apart.
  for (const action of ["unsubscribe", "erase"]) {
    const unknown = await owner.request.post(
      `/t/${slug}/newsletter/${action}`,
      { form: { subscriber_id: "unknown" }, maxRedirects: 0 },
    );
    expect(unknown.status()).toBe(303);
    expect(unknown.headers()["location"]).toContain(
      "error=newsletter_not_found",
    );
  }

  await ownerContext.close();
  await viewer.context.close();
  await reader.context.close();
});

test("a merchant schedules a campaign, starts it and stops it", async ({
  page,
}) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Scheduled News");
  await createTenant(page, name);
  // Marketing goes through the shop's own server: one is needed to start.
  await page.goto(`/t/${slug}/mail`);
  await page.getByLabel("Server", { exact: true }).fill("localhost");
  await page.getByLabel("Port", { exact: true }).fill("1025");
  await page.getByLabel("Security").selectOption("none");
  await page.getByLabel("Send from address").fill(`news-${slug}@example.com`);
  await page.getByRole("button", { name: "Test and save" }).click();
  await expect(page.getByRole("status")).toContainText("Test mail sent");

  await page.goto(`/t/${slug}/newsletter/campaigns/new`);
  const form = page.locator("[data-campaign-form]");
  await form.getByLabel("Subject").fill("This week at {{ shop.name }}");
  await form
    .getByLabel("Body (HTML)")
    .fill('<p>New in.</p><a href="{{ unsubscribe_url }}">Leave</a>');
  await form.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByRole("status")).toContainText("Draft saved.");
  const campaignUrl = page.url().split("?")[0];

  // Every week with no day does not hold together.
  await page.getByRole("link", { name: "Schedule…" }).click();
  const schedule = page.locator("[data-schedule-form]");
  await schedule.getByLabel("Tuesday").uncheck();
  await schedule.getByLabel("Thursday").uncheck();
  await schedule.getByRole("button", { name: "Save the schedule" }).click();
  await expect(page.getByRole("alert")).toContainText("does not hold together");

  // Tuesdays and Thursdays, 9:00 to 17:00, a batch every 30 minutes.
  await page.getByRole("link", { name: "Schedule…" }).click();
  await expect(schedule.getByLabel("Repeat")).toHaveValue("weekly");
  await schedule.getByRole("button", { name: "Save the schedule" }).click();
  await expect(page.getByRole("status")).toContainText("Schedule saved.");
  const summary = page.locator(
    "[data-campaign-schedule] [data-schedule-summary]",
  );
  await expect(summary).toContainText("Every week on Tuesdays and Thursdays");
  await expect(summary).toContainText(
    "from 09:00 to 17:00, a batch every 30 min",
  );
  await expect(summary).toContainText("UTC for those whose zone is unknown");

  await page.getByRole("link", { name: "Start the schedule…" }).click();
  await page.getByRole("button", { name: "Start the schedule" }).click();
  await expect(page.getByRole("status")).toContainText("Scheduled");
  await expect(page.locator("dd [data-campaign-status]")).toHaveCount(0);
  // Still editable, for the editions to come.
  await expect(page.locator("[data-campaign-form]")).toBeVisible();
  // The list follows a moment later.
  await expect(async () => {
    await page.goto(`/t/${slug}/newsletter/campaigns`);
    await expect(
      page.locator(
        '[data-campaign-status="scheduled"] [data-campaign-cadence]',
      ),
    ).toHaveText("weekly");
  }).toPass();

  await page.goto(campaignUrl);
  await page.getByRole("button", { name: "Stop the schedule" }).click();
  await expect(page.getByRole("status")).toContainText("Schedule stopped");
  await expect(page.locator("dd [data-campaign-status]")).toHaveAttribute(
    "data-campaign-status",
    "stopped",
  );
});
