import { expect, test, type APIRequestContext } from "@playwright/test";
import { MAILDEV, registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// A shop's own mail server. The development mailbox of compose.yml stands in
// for it (config/e2e.toml lets a shop's server be on this machine), so the
// mails it relays are told apart by their sender's address.

type Sent = { to: { address: string }[]; from: { name: string; address: string }[]; subject: string };

async function mailTo(request: APIRequestContext, email: string, subject: string): Promise<Sent> {
  let found: Sent | undefined;
  await expect(async () => {
    const all = (await (await request.get(`${MAILDEV}/email`)).json()) as Sent[];
    found = all.filter((m) => m.to.some((to) => to.address === email) && m.subject.startsWith(subject)).at(-1);
    expect(found, `no "${subject}" mail to ${email}`).toBeDefined();
  }).toPass({ timeout: 15_000 });
  return found!;
}

test("a shop sends its mails through its own server, and back through the platform's", async ({ page }) => {
  const merchant = uniqueEmail("merchant");
  await registerWithEmail(page, merchant);
  const { name, slug } = uniqueTenant("Mail Shop");
  await createTenant(page, name);

  await page.goto(`/t/${slug}/mail`);
  await expect(page.locator("[data-mail-server]")).toHaveAttribute("data-mail-server", "platform");
  await expect(page.getByLabel("Send the test mail to")).toHaveValue(merchant);

  // Nothing listens there: the test mail cannot go out, nothing is kept.
  await page.getByLabel("Server", { exact: true }).fill("localhost");
  await page.getByLabel("Port", { exact: true }).fill("1");
  await page.getByLabel("Security").selectOption("none");
  const from = uniqueEmail("orders");
  await page.getByLabel("Send from address").fill(from);
  await page.getByRole("button", { name: "Test and save" }).click();
  await expect(page.getByRole("alert")).toContainText("The test mail did not go out");
  await expect(page.locator("[data-mail-server]")).toHaveAttribute("data-mail-server", "platform");

  await page.getByLabel("Server", { exact: true }).fill("localhost");
  await page.getByLabel("Port", { exact: true }).fill("1025");
  await page.getByLabel("Security").selectOption("none");
  await page.getByLabel("Send from address").fill(from);
  await page.getByLabel("Send from name").fill(`${name} Orders`);
  await page.getByRole("button", { name: "Test and save" }).click();
  await expect(page.getByRole("status")).toContainText("Test mail sent");
  await expect(page.locator("[data-mail-server]")).toHaveAttribute("data-mail-server", "own");
  await expect(page.locator("[data-mail-from]")).toContainText(from);
  const test_mail = await mailTo(page.request, merchant, "Your shop's mail server works");
  expect(test_mail.from[0]?.address).toBe(from);

  // A storefront's key, to ask for shopper sign-in codes.
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Label").fill("Mail storefront");
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
  const own = await mailTo(page.request, first, "Your sign-in code for");
  expect(own.from[0]?.address).toBe(from);
  expect(own.from[0]?.name).toBe(`${name} Orders`);

  await page.goto(`/t/${slug}/mail`);
  await page.getByRole("button", { name: "Use the platform's server" }).click();
  await expect(page.getByRole("status")).toContainText("platform's server again");
  await expect(page.locator("[data-mail-server]")).toHaveAttribute("data-mail-server", "platform");

  const second = uniqueEmail("shopper");
  await askCode(second);
  const platform = await mailTo(page.request, second, "Your sign-in code for");
  expect(platform.from[0]?.address).toBe("no-reply@timada.localhost");
  expect(platform.from[0]?.name).toBe(name);
});
