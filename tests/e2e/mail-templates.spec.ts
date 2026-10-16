import { expect, test, type APIRequestContext } from "@playwright/test";
import { MAILDEV, registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// A shop words its own mails: the sign-in code here, sent through the
// platform's server and read back from the development mailbox.

type Sent = { to: { address: string }[]; subject: string; html: string };

async function mailTo(request: APIRequestContext, email: string, subject: string): Promise<Sent> {
  let found: Sent | undefined;
  await expect(async () => {
    const all = (await (await request.get(`${MAILDEV}/email`)).json()) as Sent[];
    found = all.filter((m) => m.to.some((to) => to.address === email) && m.subject.startsWith(subject)).at(-1);
    expect(found, `no "${subject}" mail to ${email}`).toBeDefined();
  }).toPass({ timeout: 15_000 });
  return found!;
}

test("a shop words its sign-in code mail its own way, and back", async ({ page }) => {
  const merchant = uniqueEmail("merchant");
  await registerWithEmail(page, merchant);
  const { name, slug } = uniqueTenant("Worded Shop");
  await createTenant(page, name);

  await page.goto(`/t/${slug}/mail`);
  const row = page.locator('[data-mail-template="shopper_code"]');
  await expect(row).toContainText("Built-in wording");
  await row.getByRole("link", { name: "Sign-in code" }).click();
  await expect(page.getByRole("heading", { name: "Sign-in code" })).toBeVisible();
  await expect(page.getByLabel("Subject")).toHaveValue("Your sign-in code for {{ shop.name }}");
  await expect(page.locator("[data-preview-subject]")).toHaveText(`Your sign-in code for ${name}`);
  await expect(page.locator("[data-template-variables]")).toContainText("{{ code }}");

  // An unknown variable is refused, and the draft is kept.
  await page.getByLabel("Subject").fill("Code {{ nope }}");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.locator("[data-template-error]")).toContainText("no variable {{ nope }}");
  await expect(page.getByLabel("Subject")).toHaveValue("Code {{ nope }}");

  // So is a script.
  await page.getByLabel("Subject").fill("Your {{ shop.name }} code");
  await page.getByLabel("Body (HTML)").fill("<p>{{ code }}</p><script>alert(1)</script>");
  await page.getByRole("button", { name: "Preview" }).click();
  await expect(page.locator("[data-template-error]")).toContainText("<script>");

  await page
    .getByLabel("Body (HTML)")
    .fill("<!doctype html><html><body><p>Hello from {{ shop.name }}: <b>{{ code }}</b></p></body></html>");
  await page.getByRole("button", { name: "Preview" }).click();
  await expect(page.locator("[data-preview-subject]")).toHaveText(`Your ${name} code`);
  await expect(page.frameLocator("iframe").getByText(`Hello from ${name}`)).toBeVisible();

  await page.getByRole("button", { name: "Send me a test" }).click();
  await expect(page.getByRole("status")).toContainText(`Test mail sent to ${merchant}`);
  const test_mail = await mailTo(page.request, merchant, `[Test] Your ${name} code`);
  expect(test_mail.html).toContain("Hello from");

  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Saved");
  await expect(page.locator("[data-template-history]")).toContainText("Saved by");

  // A storefront's key, to ask for shopper sign-in codes.
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Label").fill("Worded storefront");
  await page.getByRole("checkbox", { name: "store:checkout" }).first().check();
  await page.getByRole("button", { name: "Create key" }).click();
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_/);
  const askCode = async (email: string) => {
    const response = await page.request.post("/store/shoppers/code", {
      headers: { Authorization: `Bearer ${token}` },
      form: { email },
    });
    expect(response.status()).toBe(202);
  };

  const first = uniqueEmail("shopper");
  await askCode(first);
  const worded = await mailTo(page.request, first, `Your ${name} code`);
  expect(worded.html).toContain(`Hello from ${name}`);
  expect(worded.html).toMatch(/<b>\d{6}<\/b>/);

  await page.goto(`/t/${slug}/mail`);
  await expect(row).toContainText("Edited by");
  await row.getByRole("link", { name: "Sign-in code" }).click();
  await page.getByRole("button", { name: "Back to the built-in wording" }).click();
  await expect(page.getByRole("status")).toContainText("built-in wording again");
  await expect(page.getByLabel("Subject")).toHaveValue("Your sign-in code for {{ shop.name }}");

  const second = uniqueEmail("shopper");
  await askCode(second);
  await mailTo(page.request, second, `Your sign-in code for ${name}`);
});
