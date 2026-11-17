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

test("an owner words a product and a category in another language", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Worded Shop");
  await createTenant(page, name);

  // A shop read in one language has nothing to translate into.
  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill("Arbre à chat");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created");
  const productUrl = page.url().split("?")[0];
  const tabs = page.getByRole("navigation", { name: "Product sections" });
  await expect(tabs.getByRole("link", { name: "Translations" })).toHaveCount(0);

  await page.goto(`/t/${slug}/languages`);
  const languages = page.locator("form[data-shop-languages]");
  await languages.getByLabel("Main language").selectOption("fr");
  await languages.getByRole("checkbox", { name: "English" }).check();
  await languages.getByRole("checkbox", { name: "German" }).check();
  await languages.getByRole("button", { name: "Save the languages" }).click();
  await expect(page.getByRole("status")).toContainText("Languages saved");

  // The product: a card a language, the main wording where one is empty.
  await page.goto(productUrl);
  await tabs.getByRole("link", { name: "Translations" }).click();
  const english = page.locator("[data-translation=en]");
  const german = page.locator("[data-translation=de]");
  await expect(english).toHaveAttribute("data-translation-state", "none");
  await expect(english.getByLabel("Translated name")).toHaveAttribute("placeholder", "Arbre à chat");
  await english.getByLabel("Translated name").fill("Cat Tree");
  await english.getByLabel("Translated description", { exact: true }).fill("Three floors of sisal.");
  await english.getByRole("button", { name: "Save translation" }).click();
  await expect(page.getByRole("status")).toContainText("Translation saved");
  await expect(english).toHaveAttribute("data-translation-state", "set");
  await expect(german).toHaveAttribute("data-translation-state", "none");
  // Its address there was made from the name.
  await expect(english.getByLabel("Address in this language")).toHaveValue("cat-tree");
  await expect(english.getByLabel("Translated description", { exact: true })).toHaveValue("Three floors of sisal.");
  // The product itself did not move.
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Arbre à chat");

  // An address that is no slug is refused, and one given moves it.
  await english.getByLabel("Address in this language").fill("Cat Tree!");
  await english.getByRole("button", { name: "Save translation" }).click();
  await expect(page.getByRole("alert").first()).toBeVisible();
  await expect(english.getByLabel("Address in this language")).toHaveValue("cat-tree");
  await english.getByLabel("Address in this language").fill("big-cat-tree");
  await english.getByRole("button", { name: "Save translation" }).click();
  await expect(page.getByRole("status")).toContainText("Translation saved");
  await expect(english.getByLabel("Address in this language")).toHaveValue("big-cat-tree");

  // Removed, visitors read the main wording again.
  await english.getByRole("button", { name: "Remove translation" }).click();
  await expect(page.getByRole("status")).toContainText("Translation removed");
  await expect(english).toHaveAttribute("data-translation-state", "none");
  await expect(english.getByLabel("Translated name")).toHaveValue("");

  // A category has its buying guide and its questions to word too.
  await page.goto(`/t/${slug}/categories`);
  await page.getByLabel("Name").fill("Chats");
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");
  await page.locator("[data-category-translations]").click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Translations of Chats");
  const cats = page.locator("[data-translation=de]");
  await cats.getByLabel("Translated name").fill("Katzen");
  await cats.getByLabel("Translated buying guide").fill("## Wie wählen\n\nNach Größe.");
  await cats.getByLabel("Translated questions").fill("Welche Größe?\nMessen Sie Ihre Katze.");
  await cats.getByRole("button", { name: "Save translation" }).click();
  await expect(page.getByRole("status")).toContainText("Translation saved");
  await expect(cats).toHaveAttribute("data-translation-state", "set");
  await expect(cats.getByLabel("Address in this language")).toHaveValue("katzen");
  await expect(cats.getByLabel("Translated questions")).toHaveValue("Welche Größe?\nMessen Sie Ihre Katze.");
  await expect(page.locator("[data-translation=en]")).toHaveAttribute("data-translation-state", "none");
});
