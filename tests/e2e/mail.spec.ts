import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { MAILDEV, registerWithEmail, uniqueEmail } from "./email";
import { createTenant, uniqueTenant } from "./tenants";

// A shop's own mail servers. The development mailbox of compose.yml stands in
// for each of them (config/e2e.toml lets a shop's server be on this machine),
// so the mails they relay are told apart by their sender's address.

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
  await page.getByLabel("Shop mails go through").selectOption("platform");
  await page.getByRole("button", { name: "Save choices" }).click();
  await expect(page.getByRole("status")).toContainText("the shop's mails go through the platform's server");
  await expect(page.locator("[data-mail-server]")).toHaveAttribute("data-mail-server", "platform");

  const second = uniqueEmail("shopper");
  await askCode(second);
  const platform = await mailTo(page.request, second, "Your sign-in code for");
  expect(platform.from[0]?.address).toBe("no-reply@timada.localhost");
  expect(platform.from[0]?.name).toBe(name);
});

/// Adds a server on the Mail page, the development mailbox standing in for it.
async function addServer(page: Page, slug: string, server: { name: string; from: string }) {
  await page.goto(`/t/${slug}/mail`);
  await page.getByLabel("Name", { exact: true }).fill(server.name);
  await page.getByLabel("Server", { exact: true }).fill("localhost");
  await page.getByLabel("Port", { exact: true }).fill("1025");
  await page.getByLabel("Security").selectOption("none");
  await page.getByLabel("Send from address").fill(server.from);
  await page.getByRole("button", { name: "Test and save" }).click();
  await expect(page.getByRole("status")).toContainText("Test mail sent");
}

/// Sends the merchant a test of a new campaign: the shop's marketing.
async function campaignTest(page: Page, slug: string, subject: string) {
  await page.goto(`/t/${slug}/newsletter/campaigns/new`);
  const form = page.locator("[data-campaign-form]");
  await form.getByLabel("Subject").fill(subject);
  await form.getByLabel("Body (HTML)").fill('<p>New mugs.</p><p><a href="{{ unsubscribe_url }}">Leave</a></p>');
  await form.getByRole("button", { name: "Send me a test" }).click();
}

test("a shop keeps several servers and picks the one its marketing goes through", async ({ page }) => {
  const merchant = uniqueEmail("merchant");
  await registerWithEmail(page, merchant);
  const { name, slug } = uniqueTenant("Two Servers Shop");
  await createTenant(page, name);

  // The first server sends everything; the second, nothing until picked.
  const ordersFrom = uniqueEmail("orders");
  const newsFrom = uniqueEmail("news");
  await addServer(page, slug, { name: "Orders", from: ordersFrom });
  await expect(page.getByRole("status")).toContainText("mails and its marketing now go out through this server");
  await addServer(page, slug, { name: "News", from: newsFrom });
  await expect(page.getByRole("status")).toContainText("Pick below what it sends");
  const orders = page.locator("[data-mail-server-row]", { hasText: ordersFrom });
  const news = page.locator("[data-mail-server-row]", { hasText: newsFrom });
  await expect(orders.locator("[data-sends]")).toHaveCount(2);
  await expect(news.locator("[data-sends]")).toHaveCount(0);
  await expect(page.locator("[data-mail-from]")).toContainText(ordersFrom);

  await page.getByLabel("Marketing goes through").selectOption({ label: "News · localhost" });
  await page.getByRole("button", { name: "Save choices" }).click();
  await expect(page.getByRole("status")).toContainText("its marketing through News");
  await expect(orders.locator("[data-sends]")).toHaveAttribute("data-sends", "shop-mails");
  await expect(news.locator("[data-sends]")).toHaveAttribute("data-sends", "marketing");
  await expect(page.locator("[data-mail-server]")).toHaveAttribute("data-mail-server", "own");
  await expect(page.locator("[data-mail-marketing]")).toContainText(newsFrom);

  // The shop's mails keep going through Orders...
  await page.goto(`/t/${slug}/mail/templates/shopper_code/en`);
  await page.getByRole("button", { name: "Send me a test" }).click();
  await expect(page.getByRole("status")).toContainText(`Test mail sent to ${merchant}`);
  const code = await mailTo(page.request, merchant, `[Test] Your sign-in code for ${name}`);
  expect(code.from[0]?.address).toBe(ordersFrom);

  // ...while its marketing goes through News.
  await campaignTest(page, slug, "Mugs are back");
  await expect(page.getByRole("status")).toContainText(`Test mail sent to ${merchant}`);
  const campaign = await mailTo(page.request, merchant, "[Test] Mugs are back");
  expect(campaign.from[0]?.address).toBe(newsFrom);

  // Changed on its own page, through a test mail again.
  await page.goto(`/t/${slug}/mail`);
  await news.getByRole("link", { name: "News" }).click();
  await expect(page.locator("[data-mail-server-name]")).toHaveText("News");
  await expect(page.locator("[data-mail-server-sends]")).toHaveText("The shop's marketing");
  await page.getByLabel("Send from name").fill(`${name} News`);
  await page.getByRole("button", { name: "Test and save" }).click();
  await expect(page.getByRole("status")).toContainText("Test mail sent: server saved.");
  await campaignTest(page, slug, "Teapots are back");
  const renamed = await mailTo(page.request, merchant, "[Test] Teapots are back");
  expect(renamed.from[0]?.address).toBe(newsFrom);
  expect(renamed.from[0]?.name).toBe(`${name} News`);

  // Deleted, it takes the marketing with it: nothing goes out through the
  // platform's server, nor through Orders.
  await page.goto(`/t/${slug}/mail`);
  await news.getByRole("link", { name: "News" }).click();
  await page.getByRole("button", { name: "Delete this server" }).click();
  await expect(page.getByRole("status")).toContainText("Marketing waits until you pick another server for it.");
  await expect(page.locator("[data-mail-server-row]")).toHaveCount(1);
  await expect(page.locator("[data-mail-marketing]")).toHaveAttribute("data-mail-marketing", "none");
  await expect(page.locator("[data-mail-server]")).toHaveAttribute("data-mail-server", "own");
  await campaignTest(page, slug, "Spoons are back");
  await expect(page.getByRole("alert")).toContainText("shop's own mail server only");
});
