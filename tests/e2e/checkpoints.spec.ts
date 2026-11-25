import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// A shop's target and checkpoints, as far as a browser reaches them
// without a campaign three weeks old: the target is set, then changed with
// the one before kept beside it; the four checkpoints wait for the first
// campaign; the thresholds they are judged on are the shop's to set. What
// a checkpoint reads on its day, what it says and the mail its owners get
// are covered by the dashboard's own tests (dashboard/src/checkpoints/tests.rs).

test("a shop sets its target, changes it, and sets what its checkpoints are judged on", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("checkpoints"));
  const shop = uniqueTenant("Checkpoints");
  await createTenant(page, shop.name);

  await page.getByRole("link", { name: "Checkpoints", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${shop.slug}/checkpoints$`));
  await expect(page.locator("[data-target]")).toHaveAttribute("data-target", "none");
  // Counted from the first campaign: nothing yet.
  for (const day of [21, 45, 90, 120]) {
    await expect(page.locator(`[data-checkpoint="${day}"]`)).toHaveAttribute("data-checkpoint-state", "waiting");
  }
  await expect(page.locator("[data-checkpoint-decide]")).toHaveCount(0);

  const inDays = (days: number) => new Date(Date.now() + days * 86_400_000).toISOString().slice(0, 10);
  const form = page.locator("[data-target-form]");
  await form.getByLabel("Profit after ads, a month").fill("0");
  await form.getByLabel("By").fill(inDays(200));
  await form.getByRole("button", { name: "Set the target" }).click();
  await expect(page.getByRole("alert")).toContainText("A target is an amount above zero");

  await form.getByLabel("Profit after ads, a month").fill("3000");
  await form.getByLabel("By").fill(inDays(200));
  await form.getByRole("button", { name: "Set the target" }).click();
  await expect(page.getByRole("status")).toContainText("Target set.");
  await expect(page.locator("[data-target]")).toHaveAttribute("data-target", "set");
  await expect(page.locator("[data-target-amount]")).toContainText("3000.00 €");
  await expect(page.locator("[data-target-day]")).toContainText(inDays(200));
  await expect(page.locator("[data-target-history]")).toHaveCount(0);

  // Changed: the one before stays, under it.
  await form.getByLabel("Profit after ads, a month").fill("2000");
  await form.getByLabel("By").fill(inDays(300));
  await form.getByRole("button", { name: "Change the target" }).click();
  await expect(page.getByRole("status")).toContainText("Target set.");
  await expect(page.locator("[data-target-amount]")).toContainText("2000.00 €");
  await expect(page.locator("[data-target-history]")).toContainText(`3000.00 € by ${inDays(200)}`);

  // The thresholds: the defaults, then the shop's own, kept in order.
  const thresholds = page.locator("[data-checkpoint-thresholds]");
  await expect(thresholds.getByLabel("Day 21: return to go on")).toHaveValue("0.70");
  await expect(thresholds.getByLabel("Day 120: margin to go on, in %")).toHaveValue("15");
  await thresholds.getByLabel("Day 21: return to go on").fill("0,8");
  await thresholds.getByRole("button", { name: "Save the thresholds" }).click();
  await expect(page.getByRole("status")).toContainText("Thresholds saved.");
  await expect(thresholds.getByLabel("Day 21: return to go on")).toHaveValue("0.80");
  await thresholds.getByLabel("Day 21: return to go on").fill("0.10");
  await thresholds.getByRole("button", { name: "Save the thresholds" }).click();
  await expect(page.getByRole("alert")).toContainText("the bar to go on is never under the bar to stop");
  await expect(thresholds.getByLabel("Day 21: return to go on")).toHaveValue("0.80");
});
