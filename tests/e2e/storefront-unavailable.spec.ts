import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { addToCart } from "./shop";
import { StorefrontApp, mintKey, publishProduct } from "./storefront";
import { createTenant, uniqueTenant } from "./tenants";

// The shop when timada is away. The catalogue is the shop's own copy, so
// browsing goes on; what goes through the dashboard (the cart, the account,
// the forms) is told to the shopper as momentarily unavailable, never as an
// error page. The dashboard under test stays up: the shop is pointed at
// addresses nothing answers at instead. The app runs on a port of its own,
// so this file runs beside the other storefront specs.

const app = new StorefrontApp(Number(process.env.E2E_UNAVAILABLE_PORT ?? 3461));
const SHOP = app.url;

/// A port nothing listens on: connections are refused at once.
const NOWHERE = "127.0.0.1:1";

test.afterEach(() => app.stop());

test("the shop keeps selling its catalogue and says the cart is away when the dashboard is", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Away Shop");
  await createTenant(page, name);
  await publishProduct(page, slug, "Blue Mug", "12.50");
  const key = await mintKey(page, slug, true);

  // The store API answers (the catalogue follows), the dashboard does not.
  await app.start(request, { ...key, apiUrl: `http://${NOWHERE}` }, 1);

  // Browsing: the home, the product, the search all serve.
  await page.goto(SHOP);
  await expect(page.getByRole("link", { name: "Blue Mug" }).first()).toBeVisible();
  await page.getByRole("link", { name: "Blue Mug" }).first().click();
  await expect(page.getByRole("heading", { name: "Blue Mug" })).toBeVisible();

  // Adding to the cart cannot go through: the drawer opens on the notice,
  // not on an error page, and the shopper stays on the product.
  await page.getByRole("button", { name: "Ajouter au panier" }).click();
  await expect(page.locator("[data-cart-drawer].open [data-cart-unavailable]")).toBeVisible();
  await expect(page.locator("[data-cart-drawer].open [data-cart-unavailable]")).toContainText(
    "momentanément indisponible",
  );
  await expect(page.getByRole("heading", { name: "Blue Mug" })).toBeVisible();

  // Without the script, the same request is a page of the shop saying so,
  // with the status a proxy or a crawler reads.
  const response = await page.request.post(`${SHOP}/cart/add`, {
    form: { slug: "blue-mug", quantity: "1" },
    maxRedirects: 0,
  });
  expect(response.status()).toBe(503);
  expect(response.headers()["cache-control"]).toBe("no-store");
  expect(await response.text()).toContain("momentanément indisponible");
  expect(await response.text()).not.toContain("internal server error");

  // Signing in goes through the dashboard too: the form lands on the
  // notice, the shell still around it, with a way back.
  await page.goto(`${SHOP}/account/sign-in`);
  await page.getByLabel("E-mail", { exact: true }).fill("ada@example.org");
  const [signIn] = await Promise.all([
    page.waitForResponse((r) => r.request().method() === "POST"),
    page.getByRole("button", { name: "Recevoir un code" }).click(),
  ]);
  expect(signIn.status()).toBe(503);
  await expect(page.locator("[data-store-unavailable]")).toBeVisible();
  await expect(page.locator("[data-store-unavailable] [role=alert]")).toContainText(
    "momentanément indisponible",
  );
  await expect(page.getByRole("link", { name: "Retour à l'accueil" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Rechercher" })).toBeVisible();

  // The page is not one to visit on its own.
  const direct = await page.request.get(`${SHOP}/indisponible`, { maxRedirects: 0 });
  expect(direct.status()).toBe(303);
});

test("the shop opens while the store API is away and says so", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Patient Shop");
  await createTenant(page, name);
  const key = await mintKey(page, slug);

  // Nothing answers at the store's address: the shop opens all the same.
  await app.startAway(request, key, NOWHERE);
  const status = await request.get(`${SHOP}/store/status`);
  expect(status.status()).toBe(503);
  const body = (await status.json()) as { connected: boolean; products: number; subscription: { state: string } };
  expect(body.connected).toBe(false);
  expect(body.products).toBe(0);
  expect(body.subscription.state).toBe("connecting");

  // The pages serve what the copy holds: nothing yet, and no error.
  const home = await request.get(SHOP);
  expect(home.status()).toBe(200);
  await page.goto(`${SHOP}/boutique`);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  const search = await request.get(`${SHOP}/boutique?q=mug`);
  expect(search.status()).toBe(200);
});

test("a shopper with a cart in hand keeps browsing when the dashboard goes away", async ({
  page,
  request,
}) => {
  // The shop is restarted below with the dashboard gone; the browser's
  // cookies must still be read, so the cookie key is fixed across the two runs.
  process.env.STORE_COOKIE_KEY = "e2e-unavailable-spec-cookie-key-of-32-bytes";
  await registerWithEmail(page, uniqueEmail("merchant"));
  const { name, slug } = uniqueTenant("Mid-Session Shop");
  await createTenant(page, name);
  await publishProduct(page, slug, "Blue Mug", "12.50");
  const key = await mintKey(page, slug, true);
  await app.start(request, key, 1);
  await page.goto(SHOP);
  await page.getByRole("link", { name: "Blue Mug" }).first().click();
  await addToCart(page);
  await expect(page.locator("[data-cart-count]")).toHaveText("1");

  // The dashboard goes away: the same browser, its cart cookie in hand.
  await app.terminate();
  app.stop();
  await app.start(request, { ...key, apiUrl: `http://${NOWHERE}` }, 1);

  // Every page still serves; the shell says the cart is away rather than
  // showing it empty, and no total it cannot know.
  await page.goto(`${SHOP}/#cart`);
  await expect(page.getByRole("link", { name: "Blue Mug" }).first()).toBeVisible();
  await expect(page.locator("[data-cart-total]")).toBeHidden();
  await expect(page.locator("[data-cart-drawer] [data-cart-unavailable]")).toBeVisible();
  await expect(page.locator("[data-cart-empty]")).toHaveCount(0);

  // The cart's own page is the notice, with the shop around it.
  const cart = await page.request.get(`${SHOP}/cart`);
  expect(cart.status()).toBe(503);
  await page.goto(`${SHOP}/cart`);
  await expect(page.locator("[data-store-unavailable]")).toBeVisible();
  await expect(page.getByRole("link", { name: "Réessayer" })).toHaveAttribute("href", "/cart");
  await expect(page.getByRole("button", { name: "Rechercher" })).toBeVisible();
});
