import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// Languages: an owner says under /t/{slug}/languages which language the
// shop is written in and which ones it is translated into.

test("an owner names the languages the storefront is read in", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Polyglot Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Languages", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/languages$`));
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Languages");
  const form = page.locator("form[data-shop-languages]");
  await expect(form).toHaveAttribute("data-shop-languages", "none");

  await form.getByLabel("Main language").selectOption("fr");
  await form.getByRole("checkbox", { name: "English" }).check();
  await form.getByRole("checkbox", { name: "German" }).check();
  // The main one among the others counts once.
  await form.getByRole("checkbox", { name: "French" }).check();
  await form.getByRole("button", { name: "Save the languages" }).click();
  await expect(page.getByRole("status")).toContainText("Languages saved");
  await expect(form).toHaveAttribute("data-shop-languages", "set");
  await expect(form.getByLabel("Main language")).toHaveValue("fr");
  await expect(form.getByRole("checkbox", { name: "English" })).toBeChecked();
  await expect(form.getByRole("checkbox", { name: "German" })).toBeChecked();
  await expect(form.getByRole("checkbox", { name: "French" })).not.toBeChecked();

  // A language taken off, and another main one.
  await form.getByRole("checkbox", { name: "German" }).uncheck();
  await form.getByLabel("Main language").selectOption("en");
  await form.getByRole("button", { name: "Save the languages" }).click();
  await expect(page.getByRole("status")).toContainText("Languages saved");
  await expect(form.getByLabel("Main language")).toHaveValue("en");
  await expect(form.getByRole("checkbox", { name: "English" })).not.toBeChecked();
  await expect(form.getByRole("checkbox", { name: "German" })).not.toBeChecked();
});
