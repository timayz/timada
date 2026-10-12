import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

// Registering and signing in with an e-mail address. The codes are read back
// from the development mailbox of compose.yml (MailDev), whose REST API lists
// every message it received.

export const MAILDEV = process.env.MAILDEV_URL ?? "http://localhost:1080";
export const PASSWORD = "correct horse battery";

export function uniqueEmail(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.floor(Math.random() * 1_000_000)}@example.com`;
}

type MailDevMessage = {
  id: string;
  to: { address: string }[];
  subject: string;
  text?: string;
  html?: string;
  date: string;
};

async function messagesTo(request: APIRequestContext, email: string): Promise<MailDevMessage[]> {
  const response = await request.get(`${MAILDEV}/email`);
  expect(response.ok(), `the development mailbox at ${MAILDEV} must be up (make up)`).toBe(true);
  const all = (await response.json()) as MailDevMessage[];
  return all
    .filter((m) => m.to.some((t) => t.address.toLowerCase() === email.toLowerCase()))
    .sort((a, b) => new Date(a.date).getTime() - new Date(b.date).getTime());
}

/// How many messages `email` received so far; pass it to `latestCode` so a
/// code from an earlier step is never mistaken for the new one.
export async function mailCount(request: APIRequestContext, email: string): Promise<number> {
  return (await messagesTo(request, email)).length;
}

/// The 6-digit code in the newest message to `email`, once more than
/// `previous` messages have arrived (delivery is asynchronous).
export async function latestCode(request: APIRequestContext, email: string, previous = 0): Promise<string> {
  let code = "";
  await expect(async () => {
    const messages = await messagesTo(request, email);
    expect(messages.length).toBeGreaterThan(previous);
    const last = messages[messages.length - 1];
    const match = (last.text ?? last.html ?? "").match(/\b(\d{6})\b/);
    expect(match, `no code in: ${last.subject}`).not.toBeNull();
    code = match![1];
  }).toPass({ timeout: 15_000 });
  return code;
}

/// Submits `code` on the verification page.
export async function enterCode(page: Page, code: string) {
  await page.getByLabel("Code").fill(code);
  await page.locator("form", { has: page.getByLabel("Code") }).getByRole("button").click();
}

/// Creates an account from the sign-up page and confirms the address.
export async function registerWithEmail(page: Page, email: string, password = PASSWORD) {
  const before = await mailCount(page.request, email);
  await page.goto("/auth/register");
  await page.getByLabel("E-mail address").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/auth\/email\/verify/);
  await enterCode(page, await latestCode(page.request, email, before));
  await expect(page).toHaveURL(/\/$/);
}

/// Signs in from the login page: password first, then the mailed code.
export async function signInWithEmail(page: Page, email: string, password = PASSWORD) {
  const before = await mailCount(page.request, email);
  await page.goto("/auth/login");
  await page.getByLabel("E-mail address").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/auth\/email\/verify/);
  await enterCode(page, await latestCode(page.request, email, before));
  await expect(page).toHaveURL(/\/(\?.*)?$/);
}

export { test };
