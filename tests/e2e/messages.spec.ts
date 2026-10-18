import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { invite, inviteLink } from "./invitations";
import { createTenant, uniqueTenant } from "./tenants";

// Messages: what visitors sent through a shop's contact form is listed under
// /t/{slug}/messages for members with `support:read`; those with
// `support:manage` mark a message handled. A visitor sending one from the
// Chatounette example and the owner handling it is covered in
// `chatounette.spec.ts`.

test("an owner sees the messages section, empty at first", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("New Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Messages", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/messages$`));
  await expect(page.locator("[data-message-count]")).toHaveText("0 message(s)");
  await expect(page.getByText("No message here.")).toBeVisible();
  // The owner holds every permission: the page explains marking handled.
  await expect(page.getByText("mark the message handled here")).toBeVisible();
});

test("the messages page needs support:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Messages");
  await createTenant(owner, name);

  await owner.getByRole("link", { name: "Roles", exact: true }).click();
  const define = owner.locator("form", {
    has: owner.getByRole("button", { name: "Define role" }),
  });
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
  await expect(
    viewer.getByRole("link", { name: "Messages", exact: true }),
  ).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/messages`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});

test("handling a message needs support:manage", async ({
  browser,
  request,
}) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Read Messages");
  await createTenant(owner, name);

  await owner.getByRole("link", { name: "Roles", exact: true }).click();
  const define = owner.locator("form", {
    has: owner.getByRole("button", { name: "Define role" }),
  });
  await define.getByLabel("Role name").fill("reader");
  await define.getByLabel("Label").fill("Reader");
  await define.getByLabel(/See the messages shoppers sent/).check();
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role defined.");
  const readerEmail = uniqueEmail("reader");
  await owner.getByRole("link", { name: "Members", exact: true }).click();
  await invite(owner, readerEmail, "Reader");
  const link = await inviteLink(request, readerEmail);

  const readerContext = await browser.newContext();
  const reader = await readerContext.newPage();
  await registerWithEmail(reader, readerEmail);
  await reader.goto(link);
  await reader.getByRole("button", { name: "Accept the invitation" }).click();
  await expect(reader).toHaveURL(new RegExp(`/t/${slug}`));

  // Seeing the messages, without the actions column.
  await reader.getByRole("link", { name: "Messages", exact: true }).click();
  await expect(reader).toHaveURL(new RegExp(`/t/${slug}/messages$`));
  await expect(
    reader.getByRole("columnheader", { name: "Actions" }),
  ).toHaveCount(0);
  // And posting the forms directly is refused.
  for (const action of ["handle", "reopen"]) {
    const refused = await reader.request.post(`/t/${slug}/messages/${action}`, {
      form: { message_id: "unknown" },
      maxRedirects: 0,
    });
    expect(refused.status()).toBe(403);
  }
  // The owner may, and an unknown message is told apart.
  const unknown = await owner.request.post(`/t/${slug}/messages/handle`, {
    form: { message_id: "unknown" },
    maxRedirects: 0,
  });
  expect(unknown.status()).toBe(303);
  expect(unknown.headers()["location"]).toContain("error=message_not_found");

  await ownerContext.close();
  await readerContext.close();
});
