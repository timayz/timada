import type { Page } from "@playwright/test";

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
