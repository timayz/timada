import { expect, test, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

/// Opens the pages list and waits for `slug` to be on it: the list comes
/// from a read model, a moment behind the write.
async function openPages(page: Page, tenant: string, slug: string) {
  await expect(async () => {
    await page.goto(`/t/${tenant}/pages`);
    await expect(page.locator(`[data-page=${slug}]`)).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
}

// The shop's own pages: an owner writes one in Markdown under
// /t/{slug}/pages, previews it, publishes it, moves it to another address and
// deletes it.

test("an owner writes, previews and publishes a page", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Pages Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Pages", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/pages$`));
  await expect(page.getByText("No page yet.")).toBeVisible();

  await page.getByRole("link", { name: "New page" }).click();
  await page.locator("#page-title").fill("Mentions légales");
  await page.locator("#page-body").fill("## Éditeur\n\nLe site est édité par **Acme**.\n\n<script>alert(1)</script>");
  await page.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByRole("status")).toContainText("Draft saved.");
  await expect(page.locator("[data-page-status]")).toHaveAttribute("data-page-status", "Draft");
  await expect(page.locator("#page-slug")).toHaveValue("mentions-legales");

  // The preview is the cleaned HTML: the heading and bold render, the
  // script is text.
  const preview = page.frameLocator("[data-page-preview]");
  await expect(preview.locator("h1")).toHaveText("Mentions légales");
  await expect(preview.locator("h2")).toHaveText("Éditeur");
  await expect(preview.locator("strong")).toHaveText("Acme");
  await expect(preview.locator("script")).toHaveCount(0);
  await expect(preview.locator("body")).toContainText("<script>alert(1)</script>");

  // A preview of unsaved text keeps it in the editor.
  await page.locator("#page-body").fill("## Hébergeur\n\nChez nous.");
  await page.getByRole("button", { name: "Preview" }).click();
  await expect(page.getByRole("status")).toContainText("not saved yet");
  await expect(preview.locator("h2")).toHaveText("Hébergeur");
  await expect(page.locator("#page-body")).toHaveValue("## Hébergeur\n\nChez nous.");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Page saved.");
  await expect(page.locator("#page-body")).toHaveValue("## Hébergeur\n\nChez nous.");

  // Search engines, then published.
  await page.locator("#page-meta-title").fill("Mentions légales | Acme");
  await page.getByRole("button", { name: "Save for search engines" }).click();
  await expect(page.getByRole("status")).toContainText("Saved for search engines.");
  await expect(page.locator("#page-meta-title")).toHaveValue("Mentions légales | Acme");
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.getByRole("status")).toContainText("Published");
  await expect(page.locator("[data-page-status]")).toHaveAttribute("data-page-status", "Published");

  await openPages(page, slug, "mentions-legales");
  const row = page.locator("[data-page=mentions-legales]");
  await expect(row).toHaveAttribute("data-page-status", "Published");

  // A second page cannot take the address; the first moves elsewhere.
  await page.getByRole("link", { name: "New page" }).click();
  await page.locator("#page-title").fill("Legal");
  await page.locator("#page-slug").fill("mentions-legales");
  await page.locator("#page-body").fill("Kept when refused.");
  await page.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByRole("alert").first()).toContainText("Another page has this address.");
  await expect(page.locator("#page-body")).toHaveValue("Kept when refused.");

  await openPages(page, slug, "mentions-legales");
  await page.getByRole("link", { name: "Mentions légales" }).click();
  await page.locator("#page-slug").fill("legal");
  await page.getByRole("button", { name: "Move" }).click();
  await expect(page.getByRole("status")).toContainText("Address changed.");
  await expect(page.locator("#page-slug")).toHaveValue("legal");

  await page.getByRole("button", { name: "Delete page" }).click();
  await expect(page.getByRole("status")).toContainText("Page deleted");
  await expect(async () => {
    await page.goto(`/t/${slug}/pages`);
    await expect(page.getByText("No page yet.")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
});
