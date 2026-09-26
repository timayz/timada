import { expect, test } from "@playwright/test";

// A smoke test through a real browser, complementary to demo/src/tests.rs,
// which drives the same pages socket-free through `Router::handle`. Read-only
// on purpose: it runs against the demo's data/demo.db, which may well be the
// one you are developing against.

test("the home page introduces the shop", async ({ page }) => {
  await page.goto("/");

  await expect(page).toHaveTitle("Catalogue · Timada demo");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Boutique.");

  const globalnav = page.getByRole("navigation", { name: "Principal" });
  await expect(globalnav.getByRole("link", { name: /^Panier \(\d+\)$/ })).toBeVisible();
  await expect(globalnav.getByRole("link", { name: "Se connecter" })).toBeVisible();
  await expect(globalnav.getByRole("link", { name: "Administration" })).toBeVisible();
});

test("the catalogue is seeded", async ({ page }) => {
  await page.goto("/");

  // What the home page says when nothing has been seeded. If this shows up,
  // the two tests below have no catalogue to search.
  await expect(page.getByText("Aucun produit. Lancez")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Tout le catalogue." })).toBeVisible();
});

test("searching finds the curved LG monitor", async ({ page }) => {
  await page.goto("/");

  await page.getByRole("searchbox", { name: "Rechercher un produit" }).fill("ecran incurve");
  await page.getByRole("button", { name: "Chercher" }).click();

  await expect(page).toHaveURL(/\/recherche\?q=ecran\+incurve$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Recherche : ecran incurve");

  // Accent- and case-insensitive: the seeded "LG 34\" UltraWide incurvé".
  const card = page.locator("li.product").filter({ hasText: "UltraWide incurvé" });
  await expect(card).toHaveCount(1);
  await expect(card.locator(".price")).toHaveText("399,00 €");
  await expect(card.getByText("Rupture")).toBeVisible();
});

test("a product card opens its product page", async ({ page }) => {
  await page.goto("/recherche?q=Odyssey");

  const card = page.locator("li.product").first();
  const name = await card.getByRole("heading").innerText();
  await card.getByRole("link", { name }).click();

  await expect(page).toHaveURL(/\/p\/.+/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(name);
});

test("the admin asks operators to sign in", async ({ page }) => {
  await page.goto("/admin/login");

  await expect(page.getByText("Connexion")).toBeVisible();
  await expect(page.getByLabel("Email")).toBeVisible();
  await expect(page.getByLabel("Mot de passe")).toBeVisible();
  await expect(page.getByRole("button", { name: "Se connecter" })).toBeVisible();
});
