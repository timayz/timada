import { expect, type Page } from "@playwright/test";

/// Opens a tab of the product page, and waits for it: what it shows and what it
/// no longer shows are then both safe to assert.
export async function productTab(page: Page, name: string) {
  const tab = page
    .getByRole("navigation", { name: "Product sections" })
    .getByRole("link", { name });
  await tab.click();
  await expect(tab).toHaveAttribute("aria-current", "page");
}

/// Picks the option of a `<select>` whose text contains `text`: the category
/// menus prefix a sub-category with its indent, so the label is not the name.
export async function selectByText(page: Page, label: string, text: string) {
  const select = page.getByLabel(label);
  const value = await select
    .locator("option", { hasText: text })
    .first()
    .getAttribute("value");
  await select.selectOption(value ?? "");
}
