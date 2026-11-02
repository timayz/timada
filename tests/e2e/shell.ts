import { expect, type Locator, type Page } from "@playwright/test";

// The topbar's account menu: the avatar button at its right end, whose
// accessible name says who is signed in, and the menu it opens, which holds
// the sign-out button.

/// The avatar button; its name is "Signed in as <label>".
export function accountMenu(page: Page): Locator {
  return page.locator("header").getByRole("button", { name: /^Signed in as / });
}

/// Asserts the topbar says `who` is signed in.
export async function expectSignedInAs(page: Page, who: string): Promise<void> {
  const literal = who.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  await expect(accountMenu(page)).toHaveAccessibleName(new RegExp(`^Signed in as .*${literal}`));
}

/// Opens the account menu and signs this browser out.
export async function signOut(page: Page): Promise<void> {
  await accountMenu(page).click();
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
}
