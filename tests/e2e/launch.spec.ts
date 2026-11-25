import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { addShippingMethod } from "./shop";
import { createTenant, uniqueTenant } from "./tenants";

// A shop's launch path: every step between a shop with a name and a shop
// that is advertised, each done, to do or not checked, with where it is put
// right. A step timada reads turns as the shop is set up; one it cannot
// check takes a member's word, kept with who gave it. The first campaign
// waiting for the steps marked "before ads" is in campaigns.spec.ts.

test("a new shop reads its launch path, puts a step right and says another holds", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("launch"));
  const shop = uniqueTenant("Launch");
  await createTenant(page, shop.name);

  await page.getByRole("link", { name: "Launch path", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/launch$`));
  const step = (key: string) => page.locator(`[data-launch-step=${key}]`);
  await expect(page.locator("[data-launch-step]")).toHaveCount(21);
  await expect(page.locator("[data-launch-steps]")).toHaveAttribute("data-launch-steps", "21");
  // Nothing to clash or to overlap in an empty shop: two steps hold.
  await expect(page.locator("[data-launch-done]")).toHaveAttribute("data-launch-done", "2");
  await expect(page.locator("[data-launch-waits]")).toHaveAttribute("data-launch-waits", "8");
  await expect(step("study")).toHaveAttribute("data-launch-state", "todo");
  await expect(step("study").locator("[data-launch-detail]")).toHaveText("No study is linked to the shop.");
  await expect(step("company")).toHaveAttribute("data-launch-state", "todo");
  await expect(step("company").locator("[data-launch-before-ads]")).toBeVisible();
  await expect(step("study").locator("[data-launch-before-ads]")).toHaveCount(0);
  await expect(step("product_count").locator("[data-launch-detail]")).toHaveText(
    "0 products are published, of the 200 planned.",
  );

  // What timada cannot check takes a member's word, and gives it back.
  await expect(step("consent")).toHaveAttribute("data-launch-state", "unchecked");
  await step("consent").getByRole("button", { name: "I checked: it holds" }).click();
  await expect(page.getByRole("status")).toContainText("Noted: it holds.");
  await expect(step("consent")).toHaveAttribute("data-launch-state", "done");
  await expect(step("consent").locator("[data-launch-word]")).toContainText("Said by");
  await expect(page.locator("[data-launch-waits]")).toHaveAttribute("data-launch-waits", "7");
  await step("consent").getByRole("button", { name: "Take it back" }).click();
  await expect(page.getByRole("status")).toContainText("Taken back.");
  await expect(step("consent")).toHaveAttribute("data-launch-state", "unchecked");
  // A word does not make up for what timada can tell is missing.
  await expect(step("legal")).toHaveAttribute("data-launch-state", "todo");
  await expect(step("legal").getByRole("button", { name: "I checked: it holds" })).toHaveCount(0);

  // A step put right where its link leads turns by itself.
  await expect(step("shipping_returns").locator("[data-launch-detail]")).toHaveText(
    "The shop has no shipping method.",
  );
  await step("shipping_returns").getByRole("link", { name: "Shipping" }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/shipping$`));
  await addShippingMethod(page, shop.slug, "Colis suivi", "4,90", "France FR");
  await page.goto(`/t/${shop.slug}/launch`);
  await expect(step("shipping_returns").locator("[data-launch-detail]")).toHaveText(
    "The shop does not say how returns go.",
  );
  await page.goto(`/t/${shop.slug}/shipping`);
  await page.getByLabel("Return window, in days").fill("30");
  await page.getByRole("button", { name: "Set return terms" }).click();
  await expect(page.getByRole("status")).toContainText("Return terms set.");
  await page.goto(`/t/${shop.slug}/launch`);
  await expect(step("shipping_returns")).toHaveAttribute("data-launch-state", "done");

  // A smaller catalog planned.
  const targets = page.locator("[data-launch-targets]");
  await targets.getByLabel("Products in the shop").fill("20");
  await targets.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Saved.");
  await expect(step("product_count").locator("[data-launch-detail]")).toHaveText(
    "0 products are published, of the 20 planned.",
  );
});
