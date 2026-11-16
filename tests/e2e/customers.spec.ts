import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Customers: the shoppers who signed in to a shop are listed under
// /t/{slug}/customers for members with `customers:read`; those with
// `customers:manage` delete an account. A shopper signing in, showing up
// there and being deleted is covered in `storefront.spec.ts`.

test("an owner sees the customers section, empty at first", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("New Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Customers", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/customers$`));
  await expect(page.locator("[data-customer-count]")).toHaveText("0 customer(s)");
  await expect(page.getByText("No customer here.")).toBeVisible();
});

test("the customers page needs customers:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Customers");
  await createTenant(owner, name);

  await owner.getByRole("link", { name: "Roles", exact: true }).click();
  const define = owner.locator("form", { has: owner.getByRole("button", { name: "Define role" }) });
  await define.getByLabel("Role name").fill("viewer");
  await define.getByLabel("Label").fill("Viewer");
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role defined.");
  const viewerEmail = uniqueEmail("viewer");
  await owner.getByRole("link", { name: "Members", exact: true }).click();
  await invite(owner, viewerEmail, "Viewer");
  const link = await inviteLink(request, viewerEmail);

  const viewerContext = await browser.newContext();
  const viewer = await viewerContext.newPage();
  await registerWithEmail(viewer, viewerEmail);
  await viewer.goto(link);
  await viewer.getByRole("button", { name: "Accept the invitation" }).click();
  await expect(viewer).toHaveURL(new RegExp(`/t/${slug}`));
  await expect(viewer.getByRole("link", { name: "Customers", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/customers`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});

test("deleting a customer needs customers:manage", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Read Customers");
  await createTenant(owner, name);

  // The owner holds every permission: the page says what deleting does.
  await owner.goto(`/t/${slug}/customers`);
  await expect(owner.getByText("signs them out everywhere")).toBeVisible();

  // And the owner alone schedules the customers' file: every Monday, a mail
  // that says it is ready, with a link and no file.
  const schedule = owner.locator("[data-export-schedule]");
  await expect(schedule).toHaveAttribute("data-export-schedule", "none");
  await schedule.getByLabel("How often").selectOption("weekly");
  await schedule.getByLabel("E-mail address to tell").fill("Books@Example.org");
  await schedule.getByRole("button", { name: "Save the schedule" }).click();
  await expect(owner.getByRole("status")).toContainText("Scheduled export saved.");
  await expect(schedule).toHaveAttribute("data-export-schedule", "mail");
  await expect(schedule.locator("[data-export-schedule-summary]")).toContainText(
    "Every Monday, for the week before, told by mail to books@example.org.",
  );
  const scheduleAction = (await schedule.locator("form").getAttribute("action")) ?? "";

  await owner.getByRole("link", { name: "Roles", exact: true }).click();
  const define = owner.locator("form", { has: owner.getByRole("button", { name: "Define role" }) });
  await define.getByLabel("Role name").fill("support");
  await define.getByLabel("Label").fill("Support");
  await define.getByLabel(/See the shop's customer accounts/).check();
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role defined.");
  const supportEmail = uniqueEmail("support");
  await owner.getByRole("link", { name: "Members", exact: true }).click();
  await invite(owner, supportEmail, "Support");
  const link = await inviteLink(request, supportEmail);

  const supportContext = await browser.newContext();
  const support = await supportContext.newPage();
  await registerWithEmail(support, supportEmail);
  await support.goto(link);
  await support.getByRole("button", { name: "Accept the invitation" }).click();
  await expect(support).toHaveURL(new RegExp(`/t/${slug}`));

  // Seeing the customers, without the delete action nor its column.
  await support.getByRole("link", { name: "Customers", exact: true }).click();
  await expect(support).toHaveURL(new RegExp(`/t/${slug}/customers$`));
  await expect(support.getByRole("columnheader", { name: "Actions" })).toHaveCount(0);
  await expect(support.getByRole("button", { name: "Delete" })).toHaveCount(0);
  await expect(support.getByText("signs them out everywhere")).toHaveCount(0);
  // And posting the form directly is refused.
  const refused = await support.request.post(`/t/${slug}/customers/delete`, {
    form: { shopper_id: "unknown" },
    maxRedirects: 0,
  });
  expect(refused.status()).toBe(403);
  // They see that the file goes out on its own, and where; setting it is
  // the owner's alone.
  const seen = support.locator("[data-export-schedule]");
  await expect(seen).toHaveAttribute("data-export-schedule", "mail");
  await expect(seen.locator("[data-export-schedule-summary]")).toContainText("books@example.org");
  await expect(seen.getByRole("button", { name: "Save the schedule" })).toHaveCount(0);
  await expect(seen.getByRole("button", { name: "Take it off" })).toHaveCount(0);
  const notTheirs = await support.request.post(scheduleAction, {
    form: { what: "customers", every: "daily", to: "mail", mail: supportEmail },
    maxRedirects: 0,
  });
  expect(notTheirs.status()).toBe(403);

  // The owner has the file itself posted to an address instead: it is
  // signed under a secret the page shows them.
  await owner.goto(`/t/${slug}/customers`);
  await schedule.getByLabel("How often").selectOption("daily");
  await schedule.getByRole("radio", { name: "The file, posted to an address" }).check();
  await schedule.getByLabel("Address to post the file to").fill("https://books.example/in/s3cret");
  await schedule.getByRole("button", { name: "Save the schedule" }).click();
  await expect(owner.getByRole("status")).toContainText("Scheduled export saved.");
  await expect(schedule).toHaveAttribute("data-export-schedule", "url");
  await expect(schedule.locator("[data-export-schedule-summary]")).toContainText(
    "Every day, for the day before, posted to an address.",
  );
  await expect(schedule.getByLabel("Address to post the file to")).toHaveValue(
    "https://books.example/in/s3cret",
  );
  await expect(schedule.locator("[data-export-secret]")).toHaveText(/^whsec_[0-9a-f]{64}$/);
  // The address is the owner's to read, not another member's.
  await support.reload();
  await expect(seen).toHaveAttribute("data-export-schedule", "url");
  await expect(seen).not.toContainText("books.example");

  await schedule.getByRole("button", { name: "Take it off" }).click();
  await expect(owner.getByRole("status")).toContainText("Scheduled export taken off.");
  await expect(schedule).toHaveAttribute("data-export-schedule", "none");
  // Nothing scheduled: nothing for another member to see.
  await support.reload();
  await expect(seen).toHaveCount(0);

  await ownerContext.close();
  await supportContext.close();
});
