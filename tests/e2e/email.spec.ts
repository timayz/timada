import { expect, test } from "@playwright/test";
import { PASSWORD, enterCode, latestCode, mailCount, registerWithEmail, signInWithEmail, uniqueEmail } from "./email";

// The e-mail + password method end to end: the code at sign-up, the code at
// every sign-in, a wrong code, the resend throttle, and a password reset that
// signs the other browsers out.

const NEW_PASSWORD = "a brand new password";

test("register, sign in with a mailed code, then reset the password", async ({ browser }) => {
  const email = uniqueEmail("eve");
  const laptop = await browser.newContext();
  const page = await laptop.newPage();

  await registerWithEmail(page, email);
  await expect(page.locator("header").getByText("Signed in as")).toContainText(email);
  await page.goto("/auth/account");
  await expect(page.locator("section", { hasText: "E-mail and password" })).toContainText(email);
  const signIns = page.locator("section", { hasText: "Recent sign-ins" }).locator("li");
  await expect(signIns).toHaveCount(1);
  await expect(signIns.first()).toContainText("password");
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page).toHaveURL(/\/auth\/login$/);

  // A wrong password: no mail, straight back to the form.
  const before = await mailCount(page.request, email);
  await page.getByLabel("E-mail address").fill(email);
  await page.getByLabel("Password", { exact: true }).fill("not the password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/auth\/login\?error=invalid_credentials$/);
  await expect(page.getByRole("alert").first()).toContainText("wrong");
  expect(await mailCount(page.request, email)).toBe(before);

  // The right password mails a code.
  await page.getByLabel("E-mail address").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/auth\/email\/verify$/);
  await expect(page.getByText("We sent a 6-digit code to")).toContainText(email);
  const code = await latestCode(page.request, email, before);

  // A wrong code keeps the page, and another code cannot be rushed.
  await enterCode(page, code === "000000" ? "000001" : "000000");
  await expect(page).toHaveURL(/\/auth\/email\/verify\?error=invalid_code$/);
  await expect(page.getByRole("alert").first()).toContainText("not right");
  await page.getByRole("button", { name: "Send a new code" }).click();
  await expect(page.getByRole("alert").first()).toContainText("moment ago");
  await enterCode(page, code);
  await expect(page).toHaveURL(/\/$/);
  await page.goto("/auth/account");
  await expect(signIns).toHaveCount(2);

  // A second browser signs in as well.
  const phone = await browser.newContext();
  const phonePage = await phone.newPage();
  await signInWithEmail(phonePage, email);
  await expect(phonePage.locator("header").getByText("Signed in as")).toContainText(email);

  // Forgot password from the laptop: code plus new password, confirmed twice.
  const beforeReset = await mailCount(page.request, email);
  await page.goto("/auth/forgot-password");
  await page.getByLabel("E-mail address").fill(email);
  await page.getByRole("button", { name: "Send me a code" }).click();
  await expect(page).toHaveURL(/\/auth\/email\/verify\?message=/);
  await expect(page.getByRole("status")).toContainText("If an account exists");
  const resetCode = await latestCode(page.request, email, beforeReset);
  await page.getByLabel("Code").fill(resetCode);
  await page.getByLabel("New password", { exact: true }).fill(NEW_PASSWORD);
  await page.getByLabel("Confirm the new password").fill("something else");
  await page.getByRole("button", { name: "Change password" }).click();
  await expect(page.getByRole("alert").first()).toContainText("do not match");
  await page.getByLabel("Code").fill(resetCode);
  await page.getByLabel("New password", { exact: true }).fill(NEW_PASSWORD);
  await page.getByLabel("Confirm the new password").fill(NEW_PASSWORD);
  await page.getByRole("button", { name: "Change password" }).click();
  await expect(page).toHaveURL(/\/\?message=/);
  await expect(page.getByRole("status")).toContainText("Password changed");
  await expect(page.locator("header").getByText("Signed in as")).toContainText(email);

  // The phone was signed out; the old password is dead, the new one works.
  await phonePage.goto("/auth/account");
  await expect(phonePage).toHaveURL(/\/auth\/login$/);
  await phonePage.getByLabel("E-mail address").fill(email);
  await phonePage.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await phonePage.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(phonePage).toHaveURL(/\/auth\/login\?error=invalid_credentials$/);
  await signInWithEmail(phonePage, email, NEW_PASSWORD);
  await expect(phonePage.locator("header").getByText("Signed in as")).toContainText(email);

  await laptop.close();
  await phone.close();
});

test("an address that already has an account cannot register again", async ({ page }) => {
  const email = uniqueEmail("dup");
  await registerWithEmail(page, email);
  await page.getByRole("button", { name: "Sign out", exact: true }).click();

  await page.goto("/auth/register");
  await page.getByLabel("E-mail address").fill(email.toUpperCase());
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/auth\/register\?error=already_registered/);
  await expect(page.getByRole("alert").first()).toContainText("already registered");
  await expect(page.getByLabel("E-mail address")).toHaveValue(email);

  // Policy errors keep the address too.
  await page.getByLabel("E-mail address").fill(uniqueEmail("weak"));
  await page.getByLabel("Password", { exact: true }).fill("short");
  // The browser enforces minlength; bypass it to reach the server's check.
  await page.getByLabel("Password", { exact: true }).evaluate((input: HTMLInputElement) => input.removeAttribute("minlength"));
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/auth\/register\?error=weak_password/);
  await expect(page.getByRole("alert").first()).toContainText("between 8 and 128");
});
