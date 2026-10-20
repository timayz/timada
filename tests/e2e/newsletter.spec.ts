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
