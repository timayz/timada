import { expect, type APIRequestContext, type Page } from "@playwright/test";
import { type FakeStripe, stripeSignature } from "./stripe";

// The Chatounette example as a shopper drives it, shared by the specs that
// buy something there: the merchant's Stripe account in the dashboard, the
// cart drawer, the one-page checkout and Stripe's way back. The shop is in
// French; its totals are written as the library writes them (`25.00 €`),
// its catalogue prices as the shop does (`12,50€`).

/// Connects the shop's Stripe account in the dashboard (the fake Stripe of
/// stripe.ts answers for it); the address Stripe's webhooks go to.
export async function connectStripe(page: Page, slug: string): Promise<string> {
  await page.goto(`/t/${slug}/payments`);
  const webhookUrl =
    (await page.locator("[data-webhook-url]").textContent())?.trim() ?? "";
  await page.getByLabel("Publishable key").fill("pk_test_e2e");
  await page.getByLabel("Secret key").fill("sk_test_e2e");
  await page.getByLabel("Webhook signing secret").fill("whsec_e2e");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Stripe connected.");
  return webhookUrl;
}

/// Stripe telling the dashboard a payment went through, as its webhook does.
export async function paymentSucceeded(
  request: APIRequestContext,
  webhookUrl: string,
  paymentId: string,
  secret: string,
) {
  const body = JSON.stringify({
    id: `evt_${paymentId}`,
    type: "payment_intent.succeeded",
    data: { object: { id: paymentId, object: "payment_intent" } },
  });
  return request.post(webhookUrl, {
    data: body,
    headers: {
      "content-type": "application/json",
      "stripe-signature": stripeSignature(body, secret),
    },
  });
}

/// Adds the product page's pick to the cart: the cart drawer opens on it,
/// without leaving the page.
export async function addToCart(page: Page) {
  await page.getByRole("button", { name: "Ajouter au panier" }).click();
  await expect(
    page.locator("[data-cart-drawer].open .drawer-panel"),
  ).toBeVisible();
}

/// The checkout's e-mail field (the footer's newsletter form has one too).
export const emailField = (page: Page) =>
  page.getByLabel("E-mail", { exact: true });

/// The button that places the order; its label carries the total.
export const payButton = (page: Page) =>
  page.getByRole("button", { name: /^Payer ma commande/ });

/// Fills the checkout's delivery details in, as Ada in Paris.
export async function fillAddress(
  page: Page,
  email = "ada@example.org",
  line1 = "1 rue de la Paix",
) {
  await emailField(page).fill(email);
  await page.getByLabel("Nom complet").fill("Ada Lovelace");
  await page.getByLabel("Adresse", { exact: true }).fill(line1);
  await page.getByLabel("Code postal", { exact: true }).fill("75002");
  await page.getByLabel("Ville", { exact: true }).fill("Paris");
  await page.getByLabel("Pays", { exact: true }).selectOption("FR");
}

/// The checkout's details filled in (or checked kept from the last order).
export async function fillCheckout(page: Page, fill: boolean) {
  if (fill) {
    await fillAddress(page);
  } else {
    await expect(page.getByLabel("Nom complet")).toHaveValue("Ada Lovelace");
  }
}

/// From the cart to Stripe's return page: the checkout filled in (or kept
/// from the last order), the order placed and the payment confirmed by the
/// fake Stripe.js.
export async function placeOrder(page: Page, shop: string, fill: boolean) {
  await page.goto(`${shop}/cart`);
  await page.getByRole("link", { name: "Commander" }).click();
  await fillCheckout(page, fill);
  await payButton(page).click();
  await expect(page).toHaveURL(
    /\/checkout\/complete\?.*redirect_status=succeeded/,
    { timeout: 30_000 },
  );
}

/// Confirms the last payment to the dashboard, as Stripe would, and follows
/// the browser from Stripe's return page to the order's; its address.
export async function confirmPayment(
  page: Page,
  request: APIRequestContext,
  shop: string,
  stripe: FakeStripe,
  webhookUrl: string,
): Promise<string> {
  const intent = stripe.lastIntent()!;
  stripe.pay(intent.id);
  expect(
    (
      await paymentSucceeded(request, webhookUrl, intent.id, "whsec_e2e")
    ).status(),
  ).toBe(204);
  // The return page looks again on its own every two seconds; sent there
  // while it is already doing so, the browser would arrive after the order
  // page took the cart's cookies with it.
  if (!page.url().includes("/checkout/complete")) {
    await page.goto(`${shop}/checkout/complete?redirect_status=succeeded`);
  }
  await expect(page).toHaveURL(new RegExp(`${shop}/orders/[0-9a-f]{64}$`), {
    timeout: 15_000,
  });
  return page.url();
}
