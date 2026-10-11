import { expect, test } from "@playwright/test";

test("the login page is reachable and well-formed", async ({ page }) => {
  await page.goto("/auth/login");
  await expect(page.getByRole("heading", { name: "Sign in to Timada" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Sign in with a passkey" })).toBeEnabled();
  // Every page section has a heading and the alert region exists for script errors.
  await expect(page.locator("[data-auth-error]")).toHaveCount(1);
});

test("cross-site state changes are refused", async ({ request, baseURL }) => {
  const response = await request.post(`${baseURL}/auth/logout`, {
    headers: { Origin: "https://evil.example", "Sec-Fetch-Site": "cross-site" },
  });
  expect(response.status()).toBe(403);
});
