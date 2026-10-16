import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { MAILDEV, registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// A shop words its own mails: the sign-in code here, sent through the
// platform's server and read back from the development mailbox.

type Sent = { to: { address: string }[]; subject: string; html: string; text: string };

async function mailTo(request: APIRequestContext, email: string, subject: string): Promise<Sent> {
  let found: Sent | undefined;
  await expect(async () => {
    const all = (await (await request.get(`${MAILDEV}/email`)).json()) as Sent[];
    found = all.filter((m) => m.to.some((to) => to.address === email) && m.subject.startsWith(subject)).at(-1);
    expect(found, `no "${subject}" mail to ${email}`).toBeDefined();
  }).toPass({ timeout: 15_000 });
  return found!;
}

const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

// A storefront key for the shop, and a way to ask it for a shopper's
// sign-in code.
async function codeAsker(page: Page, slug: string): Promise<(email: string) => Promise<void>> {
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Label").fill("Mail storefront");
  await page.getByRole("checkbox", { name: "store:checkout" }).first().check();
  await page.getByRole("button", { name: "Create key" }).click();
  const token = (await page.locator("[data-api-key-token]").textContent())?.trim() ?? "";
  expect(token).toMatch(/^tk_/);
  return async (email: string) => {
    const response = await page.request.post("/store/shoppers/code", {
      headers: { Authorization: `Bearer ${token}` },
      form: { email },
    });
    expect(response.status()).toBe(202);
  };
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
  await expect(page.locator("[data-preview-text]")).toHaveText("Derived from the HTML.");

  // A plain-text version is checked like the HTML.
  await page.getByLabel("Plain-text version").fill("Code {{ order.reference }}");
  await page.getByRole("button", { name: "Preview" }).click();
  await expect(page.locator("[data-template-error]")).toContainText("Plain text: this mail has no variable");
  await page.getByLabel("Plain-text version").fill("Plain hello from {{ shop.name }}: {{ code }}");
  await page.getByRole("button", { name: "Preview" }).click();
  await expect(page.locator("[data-preview-text]")).toContainText(`Plain hello from ${name}: `);

  await page.getByRole("button", { name: "Send me a test" }).click();
  await expect(page.getByRole("status")).toContainText(`Test mail sent to ${merchant}`);
  const test_mail = await mailTo(page.request, merchant, `[Test] Your ${name} code`);
  expect(test_mail.html).toContain("Hello from");
  expect(test_mail.text).toContain(`Plain hello from ${name}`);

  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Saved");
  await expect(page.locator("[data-template-history]")).toContainText("Saved by");

  const askCode = await codeAsker(page, slug);

  const first = uniqueEmail("shopper");
  await askCode(first);
  const worded = await mailTo(page.request, first, `Your ${name} code`);
  expect(worded.html).toContain(`Hello from ${name}`);
  expect(worded.html).toMatch(/<b>\d{6}<\/b>/);
  expect(worded.text).toMatch(new RegExp(`Plain hello from ${name}: \\d{6}`));

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

test("a shop gives all its mails a logo, an accent colour and a footer", async ({ page }) => {
  const merchant = uniqueEmail("merchant");
  await registerWithEmail(page, merchant);
  const { name, slug } = uniqueTenant("Branded Shop");
  await createTenant(page, name);
  const askCode = await codeAsker(page, slug);

  await page.goto(`/t/${slug}/mail`);
  const look = page.locator("[data-mail-branding]");
  await look.getByLabel("Logo").setInputFiles({ name: "logo.png", mimeType: "image/png", buffer: PNG });
  await look.getByLabel("Accent colour").fill("#C0FFEE");
  await look.getByLabel("Footer").fill(`${name} SARL\n12 rue des Lilas, Paris`);
  await look.getByRole("button", { name: "Save the look" }).click();
  await expect(page.getByRole("status")).toContainText("all your mails carry this look");
  const logo = page.locator("[data-mail-logo]");
  await expect(logo).toBeVisible();
  const src = (await logo.getAttribute("src")) ?? "";
  expect((await page.request.get(src)).status()).toBe(200);
  await expect(look.getByLabel("Accent colour")).toHaveValue("#c0ffee");

  // The editor's preview carries the look too.
  await page.locator('[data-mail-template="shopper_code"]').getByRole("link").click();
  await expect(page.frameLocator("iframe").getByText(`${name} SARL`)).toBeVisible();

  const first = uniqueEmail("shopper");
  await askCode(first);
  const branded = await mailTo(page.request, first, `Your sign-in code for ${name}`);
  expect(branded.html).toContain("color: #c0ffee;");
  expect(branded.html).toContain(`${name} SARL`);
  expect(branded.html).toMatch(/<img src="[^"]+\.png"/);

  await page.goto(`/t/${slug}/mail`);
  await page.getByRole("button", { name: "Back to the plain look" }).click();
  await expect(page.getByRole("status")).toContainText("plain look");
  await expect(page.locator("[data-mail-logo]")).toHaveCount(0);

  const second = uniqueEmail("shopper");
  await askCode(second);
  const plain = await mailTo(page.request, second, `Your sign-in code for ${name}`);
  expect(plain.html).not.toContain("<img");
  expect(plain.html).not.toContain(`${name} SARL`);
});
