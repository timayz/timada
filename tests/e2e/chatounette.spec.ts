import { expect, test, type Page } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { ExampleApp, PNG, mintKey, publishProduct, publishVariedProduct } from "./storefront";
import { createTenant, uniqueTenant } from "./tenants";

// The Chatounette clone: the second example app, in French and in the
// layout of chatounette.com, booted on a small catalogue a merchant made in
// the dashboard (the real shop's catalogue is seeded from seeds/ outside
// the browser suite). It walks the home, the category bar, a category, a
// product, the cart drawer, the search and a shop page.

const app = new ExampleApp("chatounette", Number(process.env.E2E_CHATOUNETTE_PORT ?? 3457));

test.afterEach(() => app.stop());

/// Picks the option of a `<select>` whose text contains `text`.
async function selectByText(page: Page, label: string, text: string) {
  const select = page.getByLabel(label);
  const value = await select.locator("option", { hasText: text }).first().getAttribute("value");
  await select.selectOption(value ?? "");
}

/// Creates a category, under `parent` (by menu text) when given, once the
/// category list shows the parent.
async function createCategory(page: Page, slug: string, name: string, parent?: string) {
  await expect(async () => {
    await page.goto(`/t/${slug}/categories`);
    if (parent) {
      await expect(page.getByLabel("Parent")).toContainText(parent, { timeout: 1000 });
    }
  }).toPass({ timeout: 15_000 });
  await page.getByLabel("Name").fill(name);
  if (parent) {
    await selectByText(page, "Parent", parent);
  }
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");
}

/// A picture on the product whose dashboard page is open.
async function addPicture(page: Page, url: string, alt: string) {
  await page.goto(url);
  await page.getByLabel("Add a picture").setInputFiles({ name: "photo.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill(alt);
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
}

test("the Chatounette clone shows the catalogue as a French cat shop", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("chatounette"));
  const { name, slug } = uniqueTenant("Chat Clone");
  await createTenant(page, name);

  await createCategory(page, slug, "Couchage");
  await createCategory(page, slug, "Tapis", "Couchage");
  const mat = await publishProduct(page, slug, "Tapis fraîcheur", "19.90", "Tapis");
  await addPicture(page, mat, "Un tapis bleu");
  await publishProduct(page, slug, "Panier douillet", "34.90", "Couchage");
  // A collar in two colours, priced apart: what the filters are tried on.
  await publishVariedProduct(
    page,
    slug,
    "Collier étoilé",
    "Couleur",
    [
      ["Rose", "9.90"],
      ["Bleu", "59.90"],
    ],
    "Tapis",
  );

  const key = await mintKey(page, slug, true);
  await app.start(request, key, 3);

  // The home: French, the shop's own sections, the newest products, the
  // category tiles with their counts, the category bar.
  await page.goto(`${app.url}/`);
  await expect(page.locator("html")).toHaveAttribute("lang", "fr");
  await expect(page).toHaveTitle(/Chatounette/);
  await expect(page.getByRole("heading", { level: 1 })).toContainText("l'endroit parfait pour les accessoires");
  await expect(page.locator(".topbar")).toContainText("MONSTER10");
  const newest = page.locator("[data-section=nouveautes]");
  await expect(newest.getByRole("heading", { level: 2 })).toHaveText("Nouveautés");
  await expect(newest.locator("[data-product=tapis-fraicheur]")).toContainText("Tapis fraîcheur");
  await expect(newest.locator("[data-product=tapis-fraicheur] [data-price]")).toHaveText("19,90€");
  await expect(newest.locator("[data-product=tapis-fraicheur] img")).toHaveAttribute("alt", "Un tapis bleu");
  await expect(
    newest.locator("[data-product=panier-douillet]").getByRole("button", { name: "Ajouter au panier" }),
  ).toBeVisible();
  const tile = page.locator("[data-tile=couchage]");
  await expect(tile).toContainText("Couchage");
  await expect(tile).toContainText("3 articles");
  await expect(tile.locator("img")).toHaveAttribute("alt", "Un tapis bleu");
  await expect(page.locator("[data-reassure]")).toContainText("Retours sous 14 jours");
  await expect(page.locator("nav.sections [data-menu=couchage] summary")).toHaveText("Couchage");
  await expect(page.locator("nav.sections [data-menu-all]")).toHaveText("Nos produits");
  await expect(page.locator(".sitefooter")).toContainText("Conditions générales de vente");

  // A category: the breadcrumb, the sub-categories, the count and the
  // shop's sort labels.
  await page.goto(`${app.url}/c/couchage`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Couchage");
  await expect(page.locator("[data-crumbs]")).toContainText("Accueil / Couchage");
  await expect(page.locator("[data-subcategory=tapis]")).toContainText("Tapis (2)");
  await expect(page.locator("[data-results]")).toContainText("3 résultats affichés");
  await expect(page.locator("[data-sort] option")).toHaveText([
    "Tri du plus récent au plus ancien",
    "Tri par notes moyennes",
    "Tri par tarif croissant",
    "Tri par tarif décroissant",
  ]);
  await page.locator("[data-sort]").selectOption("price_desc");
  await page.getByRole("button", { name: "Trier" }).click();
  await expect(page).toHaveURL(/sort=price_desc/);
  await expect(page.locator("ul.catalog [data-product]").first()).toHaveAttribute("data-product", "collier-etoile");

  // The filters: the colours of the listing with their counts, a colour on
  // and off again, the price bands (a range reaches into every band it
  // overlaps), the sort keeping them, and nothing left.
  await page.goto(`${app.url}/c/tapis`);
  const filters = page.locator("[data-filters]");
  await expect(filters.locator("summary")).toHaveText("Afficher les filtres");
  await expect(filters).toHaveJSProperty("open", false);
  await expect(filters.locator("[data-price-band]")).toHaveCount(7);
  await expect(filters.locator("[data-price-band='0-50']")).toHaveText("De 0,00€ à 50,00€");
  await expect(filters.locator("[data-colour]")).toHaveText(["Bleu (1)", "Rose (1)"]);
  await filters.locator("summary").click();
  await filters.locator("[data-colour=rose]").click();
  await expect(page).toHaveURL(/couleur=rose/);
  await expect(page.locator("[data-results]")).toContainText("1 résultat affiché");
  await expect(page.locator("ul.catalog [data-product]")).toHaveText(/Collier étoilé/);
  await expect(filters).toHaveJSProperty("open", true);
  await expect(filters.locator("[data-colour=rose]")).toHaveAttribute("aria-current", "true");
  await expect(filters.locator("[data-colour=bleu]")).toHaveText("Bleu (1)");
  await page.locator("[data-sort]").selectOption("price_asc");
  await page.getByRole("button", { name: "Trier" }).click();
  await expect(page).toHaveURL(/sort=price_asc/);
  await expect(page).toHaveURL(/couleur=rose/);
  await filters.locator("[data-colour=rose]").click();
  await expect(page).not.toHaveURL(/couleur=/);
  await expect(page.locator("[data-results]")).toContainText("2 résultats affichés");
  await expect(filters).toHaveJSProperty("open", false);
  await filters.locator("summary").click();
  await filters.locator("[data-price-band='50-70']").click();
  await expect(page).toHaveURL(/min_price=50&max_price=70/);
  await expect(page.locator("ul.catalog [data-product]")).toHaveCount(1);
  await expect(page.locator("ul.catalog [data-product]")).toHaveAttribute("data-product", "collier-etoile");
  await expect(filters.locator("[data-price-band='50-70']")).toHaveAttribute("aria-current", "true");
  await expect(filters.locator("[data-colour]")).toHaveText(["Bleu (1)", "Rose (1)"]);
  await filters.locator("[data-price-band='70-100']").click();
  await expect(page.locator("[data-filters-empty]")).toContainText("Aucun produit ne correspond à ces filtres.");
  await expect(filters.locator("[data-colour]")).toHaveCount(0);
  await page.locator("[data-filters-clear]").click();
  await expect(page).not.toHaveURL(/min_price=/);
  await expect(page.locator("[data-results]")).toContainText("2 résultats affichés");
  await page.goto(`${app.url}/boutique?q=collier&couleur=bleu`);
  await expect(page.locator("[data-results]")).toContainText("1 résultat affiché");
  await expect(page.locator("[data-filters] [data-colour=bleu]")).toHaveAttribute("aria-current", "true");
  await page.goto(`${app.url}/boutique?q=collier&couleur=gris`);
  await expect(page.locator("[data-filters-empty]")).toBeVisible();

  // A product: the French page, then into the cart from it, and from a card.
  await page.goto(`${app.url}/p/tapis-fraicheur`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Tapis fraîcheur");
  await expect(page.locator("[data-crumbs]")).toContainText("Accueil / Couchage / Tapis / Tapis fraîcheur");
  await expect(page.locator(".buy [data-price]")).toHaveText("19,90€");
  await expect(page.locator("[data-reassure]")).toContainText("Livraison gratuite dès 65€ d'achats");
  await expect(page.getByRole("heading", { name: "Avis de nos clients" })).toBeVisible();
  await expect(page.locator("[data-cart-count]")).toBeHidden();
  await page.getByRole("button", { name: "Ajouter au panier" }).click();
  await expect(page.locator("[data-cart-drawer].open .drawer-panel")).toBeVisible();
  await expect(page.locator("[data-cart-drawer]")).toContainText("Tapis fraîcheur");
  await expect(page.locator("[data-cart-count]")).toHaveText("1");

  await page.goto(`${app.url}/boutique`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Nos produits");
  await page.locator("[data-product=panier-douillet]").getByRole("button", { name: "Ajouter au panier" }).click();
  await expect(page.locator("[data-cart-drawer].open .drawer-panel")).toBeVisible();
  await expect(page.locator("[data-cart-drawer]")).toContainText("Panier douillet");
  await expect(page.locator("[data-cart-count]")).toHaveText("2");

  // The search and the shop's own pages.
  await page.goto(`${app.url}/boutique?q=douillet`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Résultats pour « douillet »");
  await expect(page.locator("[data-results]")).toContainText("1 résultat affiché");
  await expect(page.locator("ul.catalog [data-product]")).toHaveCount(1);

  // The live search: the fragment the masthead fetches, then the dropdown
  // itself as the shopper types.
  const fragment = await page.request.get(`${app.url}/search?q=douillet`);
  expect(fragment.status()).toBe(200);
  const body = await fragment.text();
  expect(body).toContain('data-search-product="panier-douillet"');
  expect(body).not.toContain("<html");
  await page.goto(`${app.url}/`);
  const box = page.getByRole("searchbox", { name: "Rechercher un produit" });
  const drop = page.locator("[data-search-drop]");
  await expect(drop).toBeHidden();
  await box.fill("douil");
  await expect(drop.locator("[data-search-product=panier-douillet]")).toContainText("Panier douillet");
  await expect(drop.locator("[data-search-product=panier-douillet] [data-price]")).toHaveText("34,90€");
  await expect(drop.locator("[data-search-all]")).toHaveText("Voir tous les résultats (1)");
  await expect(drop.locator("[data-search-all]")).toHaveAttribute("href", "/boutique?q=douil");
  await expect(box).toHaveAttribute("aria-expanded", "true");
  await box.press("ArrowDown");
  await expect(drop.locator("[data-search-product=panier-douillet]")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(drop).toBeHidden();
  await expect(box).toBeFocused();
  await box.fill("fraicheur");
  await expect(drop.locator("[data-search-product=tapis-fraicheur] img")).toHaveAttribute("alt", "Un tapis bleu");
  await box.fill("couch");
  await expect(drop.locator("[data-search-category=couchage]")).toContainText("Couchage");
  await box.fill("livrais");
  await expect(drop).toContainText("Autres résultats");
  await expect(drop.locator("[data-search-page=politique-de-livraison]")).toContainText("Politique de livraison");
  await box.fill("zzzz");
  await expect(drop.locator("[data-search-empty]")).toContainText("Aucun résultat pour « zzzz »");
  await box.fill("z");
  await expect(drop).toBeHidden();

  await page.goto(`${app.url}/pages/politique-de-livraison`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Politique de livraison");
  await expect(page.locator("[data-page=politique-de-livraison]")).toContainText("65 €");
  expect((await page.goto(`${app.url}/pages/nothing-here`))?.status()).toBe(404);
  expect((await page.goto(`${app.url}/p/nothing-here`))?.status()).toBe(404);
});
