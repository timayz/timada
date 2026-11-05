import { expect, test, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// Announcements: an owner writes the storefront's top-bar messages under
// /t/{slug}/announcements, gives one a window, orders, hides, edits and
// deletes them. The storefront showing them is covered in
// `storefront.spec.ts`.

/// The messages of the list, in its order, as text.
async function texts(page: Page): Promise<string[]> {
  return page.locator("[data-announcement-text]").allTextContents();
}

test("an owner writes, orders, hides and deletes announcements", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Bar Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Announcements", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/announcements$`));
  await expect(page.getByText("No announcement: the storefront shows no bar.")).toBeVisible();

  // Bold and one link render; the list shows them as the bar will.
  await page.getByLabel("Message").fill("-10% maintenant avec le code **BIENVENUE10**");
  await page.getByRole("button", { name: "Add announcement" }).click();
  await expect(page.getByRole("status")).toContainText("Announcement added");
  const first = page.locator("[data-announcement]").first();
  await expect(first.locator("strong")).toHaveText("BIENVENUE10");
  await expect(first).toHaveAttribute("data-announcement-status", "Live");

  // Two links, or a link elsewhere than the shop or https, are refused.
  await page.getByLabel("Message").fill("[Un](/a) et [deux](/b)");
  await page.getByRole("button", { name: "Add announcement" }).click();
  await expect(page.getByRole("alert").first()).toContainText("links at most once");

  // A window that opens in the future is scheduled.
  await page.getByLabel("Message").fill("Soldes : [tout voir](/boutique)");
  await page.getByLabel("From (optional)").fill("2099-06-01T09:00");
  await page.getByLabel("Until (optional)").fill("2099-06-30T23:59");
  await page.getByRole("button", { name: "Add announcement" }).click();
  await expect(page.getByRole("status")).toContainText("Announcement added");
  const sale = page.locator("[data-announcement]").nth(1);
  await expect(sale).toHaveAttribute("data-announcement-status", "Scheduled");
  await expect(sale.getByRole("link", { name: "tout voir" })).toHaveAttribute("href", "/boutique");
  await expect(sale).toContainText("Mon 1 Jun 2099, 09:00 to Tue 30 Jun 2099, 23:59");

  // An end before the start is refused.
  await page.getByLabel("Message").fill("À l'envers");
  await page.getByLabel("From (optional)").fill("2099-06-30T09:00");
  await page.getByLabel("Until (optional)").fill("2099-06-01T09:00");
  await page.getByRole("button", { name: "Add announcement" }).click();
  await expect(page.getByRole("alert").first()).toContainText("the end after the start");

  // Order, hide, show.
  await sale.getByRole("button", { name: "Up" }).click();
  await expect(page.getByRole("status")).toContainText("Order saved.");
  expect(await texts(page)).toEqual(["Soldes : tout voir", "-10% maintenant avec le code BIENVENUE10"]);
  const promo = page.locator("[data-announcement]").nth(1);
  await promo.getByRole("button", { name: "Hide" }).click();
  await expect(page.getByRole("status")).toContainText("Announcement hidden.");
  await expect(promo).toHaveAttribute("data-announcement-status", "Hidden");
  await promo.getByRole("button", { name: "Show" }).click();
  await expect(promo).toHaveAttribute("data-announcement-status", "Live");

  // Edit: the form holds the message and its window in the shop's time.
  await page.locator("[data-announcement]").first().getByRole("link", { name: "Edit" }).click();
  await expect(page.getByLabel("Message")).toHaveValue("Soldes : [tout voir](/boutique)");
  await expect(page.getByLabel("From (optional)")).toHaveValue("2099-06-01T09:00");
  await expect(page.locator("[data-announcement-preview] a")).toHaveText("tout voir");
  await page.getByLabel("Message").fill("Soldes d'été : [tout voir](/boutique)");
  await page.getByLabel("From (optional)").fill("");
  await page.getByLabel("Until (optional)").fill("");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Announcement saved");
  const edited = page.locator("[data-announcement]").first();
  await expect(edited).toHaveAttribute("data-announcement-status", "Live");
  await expect(edited).toContainText("Always");

  await edited.getByRole("button", { name: "Delete" }).click();
  await expect(page.getByRole("status")).toContainText("Announcement deleted.");
  expect(await texts(page)).toEqual(["-10% maintenant avec le code BIENVENUE10"]);
});
