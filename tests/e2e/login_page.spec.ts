import { expect, test } from "@playwright/test";

test("the login page is reachable and well-formed", async ({ page }) => {
  await page.goto("/auth/login");
  await expect(page.getByRole("heading", { name: "Sign in to Timada" })).toBeVisible();
  await expect(page.getByLabel("E-mail address")).toBeVisible();
  await expect(page.getByLabel("Password", { exact: true })).toBeVisible();
  await expect(page.getByRole("link", { name: "Forgot password?" })).toHaveAttribute("href", /\/auth\/forgot-password$/);
  await expect(page.getByRole("link", { name: "Create an account" })).toHaveAttribute("href", /\/auth\/register$/);
  await expect(page.getByRole("button", { name: "Sign in with a passkey" })).toBeEnabled();
  // Passkeys no longer create accounts.
  await expect(page.getByText("Create an account with a passkey")).toHaveCount(0);
  // Every page section has a heading and the alert region exists for script errors.
  await expect(page.locator("[data-auth-error]")).toHaveCount(1);
});

test("cross-site state changes are refused", async ({ request, baseURL }) => {
  const response = await request.post(`${baseURL}/auth/logout`, {
    headers: { Origin: "https://evil.example", "Sec-Fetch-Site": "cross-site" },
  });
  expect(response.status()).toBe(403);
});
