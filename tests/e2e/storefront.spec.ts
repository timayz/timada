import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type Page,
} from "@playwright/test";
import {
  latestCode,
  mailCount,
  mailCountOf,
  mailTo,
  registerWithEmail,
  uniqueEmail,
} from "./email";
import {
  StorefrontApp,
  PNG,
  mintKey,
  publishProduct,
  publishVariedProduct,
} from "./storefront";
import { selectByText } from "./forms";
import { addShippingMethod, addToCart } from "./shop";
import { createTenant, uniqueTenant } from "./tenants";

// The storefront, the app on the storefront library, in French, booted on a
// small catalogue a merchant made in the dashboard (a real-sized catalogue is
// seeded from seeds/ outside the browser suite). It walks the home, the
// category bar, a category, a product, the cart drawer, the search and a shop
// page.

const app = new StorefrontApp(Number(process.env.E2E_STOREFRONT_PORT ?? 3457));

test.afterEach(() => app.stop());

/// Creates a category, under `parent` (by menu text) when given, once the
/// category list shows the parent.
async function createCategory(
  page: Page,
  slug: string,
  name: string,
  parent?: string,
) {
  await expect(async () => {
    await page.goto(`/t/${slug}/categories`);
    if (parent) {
      await expect(page.getByLabel("Parent")).toContainText(parent, {
        timeout: 1000,
      });
    }
  }).toPass({ timeout: 15_000 });
  await page.getByLabel("Name").fill(name);
  if (parent) {
    await selectByText(page, "Parent", parent);
  }
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");
}

/// Writes the text of one of the shop's own pages in the dashboard and
/// publishes it. Returns its dashboard page.
async function publishPage(
  page: Page,
  slug: string,
  title: string,
  body: string,
): Promise<string> {
  await page.goto(`/t/${slug}/pages/new`);
  await page.locator("#page-title").fill(title);
  await page.locator("#page-body").fill(body);
  await page.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByRole("status")).toContainText("Draft saved.");
  const url = page.url().split("?")[0];
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.getByRole("status")).toContainText("Published");
  return url;
}

/// Adds a message to the storefront's top bar in the dashboard.
async function addAnnouncement(page: Page, slug: string, text: string) {
  await page.goto(`/t/${slug}/announcements`);
  await page.getByLabel("Message").fill(text);
  await page.getByRole("button", { name: "Add announcement" }).click();
  await expect(page.getByRole("status")).toContainText("Announcement added");
}

/// A picture on the product whose dashboard page is open.
async function addPicture(page: Page, url: string, alt: string) {
  await page.goto(`${url}?tab=pictures`);
  await page
    .getByLabel("Add a picture")
    .setInputFiles({ name: "photo.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill(alt);
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
}

test("the storefront shows the catalogue in French", async ({
  page,
  request,
  browser,
}) => {
  // Close to a minute on three nodes (`make e2e.cluster`).
  test.setTimeout(120_000);
  const owner = uniqueEmail("storefront");
  await registerWithEmail(page, owner);
  const { name, slug } = uniqueTenant("Storefront Shop");
  await createTenant(page, name);

  // The text of the shop's own pages, written in the back office (the shop
  // has the routes). Written before the products, so a storefront that has
  // the products has them too.
  const delivery = await publishPage(
    page,
    slug,
    "Politique de livraison",
    "Livraison offerte dès **65 €**.\n\n| Étape | Délai |\n|---|---|\n| Préparation | 48 h |",
  );
  await publishPage(
    page,
    slug,
    "Conditions générales de vente",
    "## Prix\n\nEn euros.",
  );
  await publishPage(
    page,
    slug,
    "Qui sommes-nous ?",
    "Deux amoureux des chats.",
  );

  // The top bar's messages, taking turns.
  await addAnnouncement(page, slug, "-10% maintenant avec le code **BIENVENUE10**");
  await addAnnouncement(page, slug, "Livraison offerte dès 65 € : [voir](/politique-de-livraison)");

  await createCategory(page, slug, "Couchage");
  // The longer text the shop shows below a category's products.
  await page
    .getByLabel("Buying guide, below the products")
    .fill("Comment choisir un couchage pour chat ?");
  await page.getByRole("button", { name: "Save description" }).click();
  await expect(page.getByRole("status")).toContainText("Description saved.");
  // What search engines show for it, apart from the name a shopper reads.
  await page.getByLabel("Search engines' title").fill("Couchages pour chat");
  await page.getByRole("button", { name: "Save for search engines" }).click();
  await expect(page.getByRole("status")).toContainText("Saved for search engines.");
  await expect(page.getByLabel("Search engines' title")).toHaveValue("Couchages pour chat");
  await createCategory(page, slug, "Tapis", "Couchage");
  const mat = await publishProduct(
    page,
    slug,
    "Tapis fraîcheur",
    "19.90",
    "Tapis",
  );
  await addPicture(page, mat, "Un tapis bleu");
  await page.goto(mat);
  await page.getByLabel("Search engines' title").fill("Tapis rafraîchissant pour chat");
  await page.getByLabel("Search engines' description").fill("Un tapis qui garde\nvotre chat au frais.");
  await page.getByRole("button", { name: "Save for search engines" }).click();
  await expect(page.getByRole("status")).toContainText("Saved for search engines.");
  await publishProduct(page, slug, "Panier douillet", "34.90", "Couchage");
  // A collar in two colours, priced apart: what the filters are tried on.
  const collarUrl = await publishVariedProduct(
    page,
    slug,
    "Collier étoilé",
    "Couleur",
    [
      ["Rose", "9.90"],
      ["Bleu", "69.90"],
    ],
    "Tapis",
  );
  // Who makes it, the number under the pink one's barcode, and what its
  // option is to a search engine.
  await page.goto(`${collarUrl}?tab=shopping`);
  await page.getByLabel("Brand").fill("Chatounette");
  await page.getByRole("button", { name: "Save identifiers" }).click();
  await expect(page.getByRole("status")).toContainText("Identifiers saved.");
  await page.getByLabel("GTIN of Couleur: Rose").fill("4006381333931");
  await page.locator("[data-variant-identifiers] tr").filter({ hasText: "Couleur: Rose" }).getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Variant identifiers saved.");
  await page.getByLabel("To a feed, Couleur is").selectOption("color");
  await page.getByRole("button", { name: "Save kind" }).click();
  await expect(page.getByRole("status")).toContainText("Option kind saved.");
  // The blue one, on sale at 69.90, goes down to 59.90: it may say what
  // it is reduced from, and no more than that.
  const blueRow = page.locator("[data-variant]").filter({ hasText: "Couleur: Bleu" });
  await page.goto(`${collarUrl}?tab=variants`);
  await page.getByLabel("Price of Couleur: Bleu").fill("59.90");
  await page.getByLabel("Price of Couleur: Bleu").press("Enter");
  await expect(page.getByRole("status")).toContainText("Variant price set.");
  await page.getByLabel("Reduced from, for Couleur: Bleu").fill("79.90");
  await blueRow.getByRole("button", { name: "Apply reduction" }).click();
  await expect(page).toHaveURL(/error=compare_at_above_earlier/);
  await page.getByLabel("Reduced from, for Couleur: Bleu").fill("69.90");
  await blueRow.getByRole("button", { name: "Apply reduction" }).click();
  await expect(page.getByRole("status")).toContainText("Reduction saved.");

  const key = await mintKey(page, slug, true);
  await app.start(request, key, 3);

  // The home: French, the shop's own sections, the newest products, the
  // category tiles with their counts, the category bar.
  await page.goto(`${app.url}/`);
  await expect(page.locator("html")).toHaveAttribute("lang", "fr");
  await expect(page).toHaveTitle(/Ma boutique/);
  await expect(page.getByRole("heading", { level: 1 })).toContainText(
    "Présentez votre boutique",
  );
  // The first message shows, the second takes its turn after five
  // seconds, and Pause holds the bar.
  const bar = page.getByRole("region", { name: "Annonces" });
  const messages = bar.locator("[data-announcement]");
  await expect(messages.first().locator("strong")).toHaveText("BIENVENUE10");
  await expect(messages.nth(1)).toBeHidden();
  await expect(messages.nth(1)).toBeVisible({ timeout: 8_000 });
  await expect(messages.first()).toBeHidden();
  await expect(messages.nth(1).getByRole("link", { name: "voir" })).toHaveAttribute(
    "href",
    "/politique-de-livraison",
  );
  await bar.getByRole("button", { name: "Pause" }).click();
  await expect(bar.getByRole("button", { name: "Reprendre" })).toHaveAttribute("aria-pressed", "true");
  const newest = page.locator("[data-section=nouveautes]");
  await expect(newest.getByRole("heading", { level: 2 })).toHaveText(
    "Nouveautés",
  );
  await expect(newest.locator("[data-product=tapis-fraicheur]")).toContainText(
    "Tapis fraîcheur",
  );
  await expect(
    newest.locator("[data-product=tapis-fraicheur] [data-price]"),
  ).toHaveText("19,90€");
  await expect(
    newest.locator("[data-product=tapis-fraicheur] img"),
  ).toHaveAttribute("alt", "Un tapis bleu");
  await expect(
    newest
      .locator("[data-product=panier-douillet]")
      .getByRole("button", { name: "Ajouter au panier" }),
  ).toBeVisible();
  const tile = page.locator("[data-tile=couchage]");
  await expect(tile).toContainText("Couchage");
  await expect(tile).toContainText("3 articles");
  await expect(tile.locator("img")).toHaveAttribute("alt", "Un tapis bleu");
  await expect(page.locator("[data-reassure]")).toContainText(
    "Retours sous 14 jours",
  );
  await expect(
    page.locator("nav.sections [data-menu=couchage] summary"),
  ).toHaveText("Couchage");
  await expect(page.locator("nav.sections [data-menu-all]")).toHaveText(
    "Nos produits",
  );
  await expect(
    page
      .locator(".sitefooter")
      .getByRole("link", { name: "Conditions générales de vente" }),
  ).toHaveAttribute("href", "/conditions-generales-de-vente");
  await expect(
    page
      .locator(".sitefooter")
      .getByRole("link", { name: "Qui sommes-nous ?" }),
  ).toHaveAttribute("href", "/qui-sommes-nous");

  // A category: the breadcrumb, the sub-categories, the count and the
  // shop's sort labels.
  await page.goto(`${app.url}/c/couchage`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Couchage");
  // Search engines read the merchant's title; with no description written
  // anywhere, the page gives them none.
  await expect(page).toHaveTitle("Couchages pour chat – Ma boutique");
  await expect(page.locator("meta[name=description]")).toHaveCount(0);
  await expect(page.locator("[data-crumbs]")).toContainText(
    "Accueil / Couchage",
  );
  await expect(page.locator("[data-subcategory=tapis]")).toContainText(
    "Tapis (2)",
  );
  await expect(page.locator("[data-category-body]")).toContainText(
    "Comment choisir un couchage pour chat ?",
  );
  await expect(page.locator("[data-results]")).toContainText(
    "3 résultats affichés",
  );
  await expect(page.locator("[data-sort] option")).toHaveText([
    "Tri par popularité",
    "Tri du plus récent au plus ancien",
    "Tri par notes moyennes",
    "Tri par tarif croissant",
    "Tri par tarif décroissant",
  ]);
  await page.locator("[data-sort]").selectOption("price_desc");
  await page.getByRole("button", { name: "Trier" }).click();
  await expect(page).toHaveURL(/sort=price_desc/);
  await expect(
    page.locator("ul.catalog [data-product]").first(),
  ).toHaveAttribute("data-product", "collier-etoile");

  // The filters: the colours of the listing with their counts, a colour on
  // and off again, the price bands (a range reaches into every band it
  // overlaps), the sort keeping them, and nothing left.
  await page.goto(`${app.url}/c/tapis`);
  const filters = page.locator("[data-filters]");
  await expect(filters.locator("summary")).toHaveText("Afficher les filtres");
  await expect(filters).toHaveJSProperty("open", false);
  await expect(filters.locator("[data-price-band]")).toHaveCount(7);
  await expect(filters.locator("[data-price-band='0-50']")).toHaveText(
    "De 0,00€ à 50,00€",
  );
  await expect(filters.locator("[data-filter=couleur]")).toHaveText([
    "Bleu (1)",
    "Rose (1)",
  ]);
  await filters.locator("summary").click();
  await filters.locator("[data-filter-value=rose]").click();
  await expect(page).toHaveURL(/couleur=rose/);
  await expect(page.locator("[data-results]")).toContainText(
    "1 résultat affiché",
  );
  await expect(page.locator("ul.catalog [data-product]")).toHaveText(
    /Collier étoilé/,
  );
  await expect(filters).toHaveJSProperty("open", true);
  await expect(filters.locator("[data-filter-value=rose]")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(filters.locator("[data-filter-value=bleu]")).toHaveText("Bleu (1)");
  await page.locator("[data-sort]").selectOption("price_asc");
  await page.getByRole("button", { name: "Trier" }).click();
  await expect(page).toHaveURL(/sort=price_asc/);
  await expect(page).toHaveURL(/couleur=rose/);
  await filters.locator("[data-filter-value=rose]").click();
  await expect(page).not.toHaveURL(/couleur=/);
  await expect(page.locator("[data-results]")).toContainText(
    "2 résultats affichés",
  );
  await expect(filters).toHaveJSProperty("open", false);
  await filters.locator("summary").click();
  await filters.locator("[data-price-band='50-70']").click();
  await expect(page).toHaveURL(/min_price=50&max_price=70/);
  await expect(page.locator("ul.catalog [data-product]")).toHaveCount(1);
  await expect(page.locator("ul.catalog [data-product]")).toHaveAttribute(
    "data-product",
    "collier-etoile",
  );
  await expect(filters.locator("[data-price-band='50-70']")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(filters.locator("[data-filter=couleur]")).toHaveText([
    "Bleu (1)",
    "Rose (1)",
  ]);
  await filters.locator("[data-price-band='70-100']").click();
  await expect(page.locator("[data-filters-empty]")).toContainText(
    "Aucun produit ne correspond à ces filtres.",
  );
  await expect(filters.locator("[data-filter=couleur]")).toHaveCount(0);
  await page.locator("[data-filters-clear]").click();
  await expect(page).not.toHaveURL(/min_price=/);
  await expect(page.locator("[data-results]")).toContainText(
    "2 résultats affichés",
  );
  await page.goto(`${app.url}/boutique?q=collier&couleur=bleu`);
  await expect(page.locator("[data-results]")).toContainText(
    "1 résultat affiché",
  );
  await expect(
    page.locator("[data-filters] [data-filter-value=bleu]"),
  ).toHaveAttribute("aria-current", "true");
  await page.goto(`${app.url}/boutique?q=collier&couleur=gris`);
  await expect(page.locator("[data-filters-empty]")).toBeVisible();

  // A product: the French page, then into the cart from it, and from a card.
  await page.goto(`${app.url}/p/tapis-fraicheur`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Tapis fraîcheur",
  );
  await expect(page).toHaveTitle("Tapis rafraîchissant pour chat – Ma boutique");
  await expect(page.locator("meta[name=description]")).toHaveAttribute(
    "content",
    "Un tapis qui garde votre chat au frais.",
  );
  await expect(page.locator("[data-crumbs]")).toContainText(
    "Accueil / Couchage / Tapis / Tapis fraîcheur",
  );
  await expect(page.locator(".buy [data-price]")).toHaveText("19,90€");
  await expect(page.locator("[data-reassure]")).toContainText(
    "Livraison avec suivi",
  );
  await expect(
    page.getByRole("heading", { name: "Avis de nos clients" }),
  ).toBeVisible();
  await expect(page.locator("[data-cart-count]")).toBeHidden();
  await page.getByRole("button", { name: "Ajouter au panier" }).click();
  await expect(
    page.locator("[data-cart-drawer].open .drawer-panel"),
  ).toBeVisible();
  await expect(page.locator("[data-cart-drawer]")).toContainText(
    "Tapis fraîcheur",
  );
  await expect(page.locator("[data-cart-count]")).toHaveText("1");

  await page.goto(`${app.url}/boutique`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Nos produits",
  );
  await page
    .locator("[data-product=panier-douillet]")
    .getByRole("button", { name: "Ajouter au panier" })
    .click();
  await expect(
    page.locator("[data-cart-drawer].open .drawer-panel"),
  ).toBeVisible();
  await expect(page.locator("[data-cart-drawer]")).toContainText(
    "Panier douillet",
  );
  await expect(page.locator("[data-cart-count]")).toHaveText("2");

  // The search and the shop's own pages.
  await page.goto(`${app.url}/boutique?q=douillet`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Résultats pour « douillet »",
  );
  await expect(page.locator("[data-results]")).toContainText(
    "1 résultat affiché",
  );
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
  await expect(
    drop.locator("[data-search-product=panier-douillet]"),
  ).toContainText("Panier douillet");
  await expect(
    drop.locator("[data-search-product=panier-douillet] [data-price]"),
  ).toHaveText("34,90€");
  await expect(drop.locator("[data-search-all]")).toHaveText(
    "Voir tous les résultats (1)",
  );
  await expect(drop.locator("[data-search-all]")).toHaveAttribute(
    "href",
    "/boutique?q=douil",
  );
  await expect(box).toHaveAttribute("aria-expanded", "true");
  await box.press("ArrowDown");
  await expect(
    drop.locator("[data-search-product=panier-douillet]"),
  ).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(drop).toBeHidden();
  await expect(box).toBeFocused();
  await box.fill("fraicheur");
  await expect(
    drop.locator("[data-search-product=tapis-fraicheur] img"),
  ).toHaveAttribute("alt", "Un tapis bleu");
  await box.fill("couch");
  await expect(drop.locator("[data-search-category=couchage]")).toContainText(
    "Couchage",
  );
  await box.fill("livrais");
  await expect(drop).toContainText("Autres résultats");
  await expect(
    drop.locator("[data-search-page=politique-de-livraison]"),
  ).toContainText("Politique de livraison");
  await expect(
    drop.locator("[data-search-page=politique-de-livraison]"),
  ).toHaveAttribute("href", "/politique-de-livraison");
  await box.fill("suppor");
  await expect(drop.locator("[data-search-page=support-client]")).toContainText(
    "Support client",
  );
  await expect(
    drop.locator("[data-search-page=support-client]"),
  ).toHaveAttribute("href", "/support-client");
  await box.fill("zzzz");
  await expect(drop.locator("[data-search-empty]")).toContainText(
    "Aucun résultat pour « zzzz »",
  );
  await box.fill("z");
  await expect(drop).toBeHidden();

  await page.goto(`${app.url}/politique-de-livraison`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Politique de livraison",
  );
  const article = page.locator("[data-page=politique-de-livraison]");
  await expect(article.locator("strong")).toHaveText("65 €");
  await expect(article.locator("td").last()).toHaveText("48 h");

  // An edit in the back office reaches the storefront; unpublished, the
  // page is gone.
  await page.goto(delivery);
  await page.locator("#page-body").fill("Livraison offerte dès **50 €**.");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Page saved.");
  await expect(async () => {
    await page.goto(`${app.url}/politique-de-livraison`);
    await expect(article.locator("strong")).toHaveText("50 €", {
      timeout: 1000,
    });
  }).toPass({ timeout: 15_000 });
  // What search engines are told: where the sitemap is and what to leave
  // alone, then the files, with whole addresses.
  const robots = await request.get(`${app.url}/robots.txt`);
  expect(robots.headers()["content-type"]).toContain("text/plain");
  const rules = await robots.text();
  expect(rules).toContain("User-agent: *\nDisallow: /account\nDisallow: /cart\n");
  expect(rules).toContain("Disallow: /*?*couleur=\n");
  expect(rules).toContain(`Sitemap: ${app.url}/sitemap.xml\n`);
  const sitemap = await request.get(`${app.url}/sitemap.xml`);
  expect(sitemap.headers()["content-type"]).toContain("application/xml");
  const files = await sitemap.text();
  expect(files).toContain(`<sitemap><loc>${app.url}/sitemap-pages.xml</loc></sitemap>`);
  expect(files).toContain(`<sitemap><loc>${app.url}/sitemap-products.xml?part=1</loc></sitemap>`);
  expect(files).not.toContain("part=2");
  const listed = await (await request.get(`${app.url}/sitemap-pages.xml`)).text();
  expect(listed).toContain(`<url><loc>${app.url}/</loc></url>`);
  expect(listed).toContain(`<url><loc>${app.url}/boutique</loc></url>`);
  expect(listed).toMatch(new RegExp(`<url><loc>${app.url}/c/couchage</loc><lastmod>\\d{4}-\\d{2}-\\d{2}</lastmod></url>`));
  expect(listed).toContain(`<loc>${app.url}/c/tapis</loc>`);
  // A page is listed once its text is published, at the shop's address for it.
  expect(listed).toContain(`<loc>${app.url}/politique-de-livraison</loc>`);
  expect(listed).not.toContain("mentions-legales");
  const sold = await (await request.get(`${app.url}/sitemap-products.xml?part=1`)).text();
  expect(sold).toMatch(new RegExp(`<url><loc>${app.url}/p/tapis-fraicheur</loc><lastmod>\\d{4}-\\d{2}-\\d{2}</lastmod></url>`));
  expect(sold).toContain(`<loc>${app.url}/p/collier-etoile</loc>`);
  expect((await request.get(`${app.url}/sitemap-products.xml?part=2`)).status()).toBe(404);
  // A page a search engine has no use for says so; a listing does once it
  // is narrowed or sorted, not as it is.
  const noindex = page.locator("meta[name=robots][content=noindex]");
  await page.goto(`${app.url}/c/tapis`);
  await expect(noindex).toHaveCount(0);
  await page.goto(`${app.url}/c/tapis?sort=price_asc`);
  await expect(noindex).toHaveCount(1);
  await page.goto(`${app.url}/c/tapis?couleur=rose`);
  await expect(noindex).toHaveCount(1);
  await page.goto(`${app.url}/boutique`);
  await expect(noindex).toHaveCount(0);
  await page.goto(`${app.url}/boutique?q=collier`);
  await expect(noindex).toHaveCount(1);
  await page.goto(`${app.url}/cart`);
  await expect(noindex).toHaveCount(1);

  // Every page names the one address search engines keep for it, draws a
  // card for a shared link, and says what it shows as data.
  const canonical = page.locator("link[rel=canonical]");
  const meta = (property: string) => page.locator(`meta[property="${property}"]`);
  const data = async () =>
    (await page.locator('script[type="application/ld+json"]').allTextContents()).map((text) => JSON.parse(text));
  await page.goto(`${app.url}/`);
  await expect(canonical).toHaveAttribute("href", `${app.url}/`);
  await expect(meta("og:type")).toHaveAttribute("content", "website");
  await expect(meta("og:site_name")).toHaveAttribute("content", "Ma boutique");
  expect((await data()).map((d) => d["@type"])).toEqual(["Organization", "WebSite"]);
  // A listing sorted or filtered names the plain one.
  await page.goto(`${app.url}/c/tapis?sort=price_asc&couleur=rose`);
  await expect(canonical).toHaveAttribute("href", `${app.url}/c/tapis`);
  const [trail] = await data();
  expect(trail["@type"]).toBe("BreadcrumbList");
  expect(trail.itemListElement.map((item: { name: string; item: string }) => [item.name, item.item])).toEqual([
    ["Accueil", `${app.url}/`],
    ["Couchage", `${app.url}/c/couchage`],
    ["Tapis", `${app.url}/c/tapis`],
  ]);
  expect(rules).toContain("Disallow: /*?*sort=\n");
  // A product sold as is: one offer, its picture at the page's size.
  await page.goto(`${app.url}/p/tapis-fraicheur`);
  await expect(canonical).toHaveAttribute("href", `${app.url}/p/tapis-fraicheur`);
  await expect(meta("og:type")).toHaveAttribute("content", "product");
  await expect(meta("og:title")).toHaveAttribute("content", "Tapis rafraîchissant pour chat");
  await expect(meta("og:image")).toHaveAttribute("content", /\/1200\/.+\.webp$/);
  await expect(meta("product:price:amount")).toHaveAttribute("content", "19.90");
  await expect(page.locator('meta[name="twitter:card"]')).toHaveAttribute("content", "summary_large_image");
  const [matData, matTrail] = await data();
  expect(matData["@type"]).toBe("Product");
  expect(matData.name).toBe("Tapis fraîcheur");
  expect(matData.description).toBe("Un tapis qui garde votre chat au frais.");
  expect(matData.offers).toMatchObject({
    url: `${app.url}/p/tapis-fraicheur`,
    price: "19.90",
    priceCurrency: "EUR",
    availability: "https://schema.org/InStock",
  });
  expect(matData.aggregateRating).toBeUndefined();
  expect(matTrail.itemListElement).toHaveLength(4);
  // A product with variants: a group, and an address for each variant,
  // which opens the page on it and still names the product's address.
  await page.goto(`${app.url}/p/collier-etoile`);
  await expect(page.locator("[data-brand]")).toHaveText("Chatounette");
  const [collar] = await data();
  expect(collar["@type"]).toBe("ProductGroup");
  expect(collar.brand).toEqual({ "@type": "Brand", name: "Chatounette" });
  expect(collar.variesBy).toEqual(["https://schema.org/color"]);
  expect(collar.hasVariant.map((v: { gtin?: string; color: string }) => [v.color, v.gtin])).toEqual([
    ["Rose", "4006381333931"],
    ["Bleu", undefined],
  ]);
  expect(collar.hasVariant.map((v: { name: string; offers: { price: string } }) => [v.name, v.offers.price])).toEqual([
    ["Collier étoilé – Rose", "9.90"],
    ["Collier étoilé – Bleu", "59.90"],
  ]);
  const blue: string = collar.hasVariant[1].offers.url;
  expect(blue).toMatch(new RegExp(`^${app.url}/p/collier-etoile\\?v=`));
  await expect(page.locator("[data-variants] a").nth(1)).toHaveAttribute("href", blue.slice(app.url.length));
  expect(collar.hasVariant[1].offers.priceSpecification).toMatchObject({
    priceType: "https://schema.org/StrikethroughPrice",
    price: "69.90",
  });
  expect(collar.hasVariant[0].offers.priceSpecification).toBeUndefined();
  await page.goto(blue);
  await expect(page.locator("[data-shown-variant]")).toHaveText("59,90€");
  await expect(page.locator("s[data-compare-at]")).toHaveText("Ancien prix : 69,90€");
  await expect(page.locator("select[data-option=Couleur]")).toHaveValue("Bleu");
  await expect(canonical).toHaveAttribute("href", `${app.url}/p/collier-etoile`);
  await expect(meta("product:price:amount")).toHaveAttribute("content", "59.90");
  await expect(noindex).toHaveCount(0);

  // A page the merchant has not written yet is a 404, and the old
  // `/pages/…` addresses are gone.
  expect((await page.goto(`${app.url}/mentions-legales`))?.status()).toBe(404);
  expect(
    (await page.goto(`${app.url}/pages/politique-de-livraison`))?.status(),
  ).toBe(404);
  expect((await page.goto(`${app.url}/p/nothing-here`))?.status()).toBe(404);

  // The support page: the form beside the questions, reached from the
  // masthead and the footer; the static slug it had is gone.
  expect((await page.goto(`${app.url}/pages/support-client`))?.status()).toBe(
    404,
  );
  await page.goto(`${app.url}/`);
  await expect(page.locator(".masthead .quick-support")).toHaveAttribute(
    "href",
    "/support-client",
  );
  await page
    .locator(".sitefooter")
    .getByRole("link", { name: "Support client" })
    .click();
  await expect(page).toHaveURL(`${app.url}/support-client`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Support client : contact@example.com",
  );
  await expect(
    page.getByRole("heading", { level: 2, name: "Nous contacter" }),
  ).toBeVisible();
  const faq = page.locator("[data-faq] details");
  await expect(faq).toHaveCount(7);
  await expect(faq.first()).toHaveAttribute("open", "");
  await expect(faq.first()).toContainText("Stripe");
  await expect(faq.nth(2)).not.toHaveAttribute("open", "");
  await expect(faq.nth(2).locator("summary")).toHaveText(
    "Quel est le délai de livraison prévu ?",
  );
  await expect(page.locator("[data-reassure]")).toContainText("Service client");

  // A visitor, browsing in French, writes. Too short a message is refused
  // with the shop's wording, and nothing is sent; a proper one is told sent.
  const visitorContext = await browser.newContext({ locale: "fr-FR" });
  const visitor = await visitorContext.newPage();
  await visitor.goto(`${app.url}/support-client`);
  const form = visitor.locator("[data-support-form]");
  await form.getByLabel("Prénom").fill("Ada");
  await form.getByLabel("Nom", { exact: true }).fill("Lovelace");
  await form.getByLabel("Email").fill("ada@example.com");
  await form.getByLabel("Message").fill("Bonjour !");
  // The browser's own minlength check would stop the form first.
  await form
    .locator("textarea")
    .evaluate((el) => el.removeAttribute("minlength"));
  await form.getByRole("button", { name: "Envoyer" }).click();
  await expect(visitor).toHaveURL(
    `${app.url}/support-client?error=message_invalid_body`,
  );
  await expect(visitor.getByRole("alert")).toContainText(
    "de 10 à 4000 caractères",
  );
  await form.getByLabel("Prénom").fill("Ada");
  await form.getByLabel("Nom", { exact: true }).fill("Lovelace");
  await form.getByLabel("Email").fill("Ada@Example.com");
  await form
    .getByLabel("Message")
    .fill("Bonjour, mon panier douillet arrive quand ?\nMerci.");
  await form.getByRole("button", { name: "Envoyer" }).click();
  await expect(visitor).toHaveURL(`${app.url}/support-client?sent=1`);
  await expect(visitor.locator("[data-sent]")).toContainText("Message envoyé");
  await expect(visitor.getByRole("alert")).toHaveCount(0);
  await visitorContext.close();

  // The owner is told, by name and with a link only: nothing the visitor
  // wrote travels by mail.
  const notice = await mailTo(request, owner, "A message is waiting for");
  expect(notice.subject).toBe(`A message is waiting for ${name}`);
  const noticeText = notice.text ?? notice.html ?? "";
  expect(noticeText).toContain("Ada");
  expect(noticeText).toContain(`/t/${slug}/messages`);
  expect(noticeText).not.toContain("ada@example.com");
  expect(noticeText).not.toContain("arrive quand");

  // The shop reads it in the back office and marks it handled.
  await expect(async () => {
    await page.goto(`/t/${slug}/messages`);
    await expect(page.locator("[data-message]")).toHaveCount(1);
  }).toPass();
  const row = page.locator("[data-message]");
  await expect(row).toHaveAttribute("data-message-status", "open");
  await expect(row).toContainText("Ada Lovelace");
  await expect(
    row.getByRole("link", { name: "ada@example.com" }),
  ).toHaveAttribute("href", "mailto:ada@example.com");
  await expect(row.locator("[data-message-language]")).toHaveText("fr");
  await expect(row.locator("[data-message-body]")).toContainText(
    "arrive quand ?",
  );
  await row.getByRole("button", { name: "Mark handled" }).click();
  await expect(page.getByRole("status")).toContainText(
    "Message marked handled",
  );
  // One notice per message, however it is handled afterwards.
  expect(await mailCountOf(request, owner, "A message is waiting for")).toBe(1);
  await expect(async () => {
    await page.goto(`/t/${slug}/messages?status=handled`);
    await expect(page.locator("[data-message]")).toHaveAttribute(
      "data-message-status",
      "handled",
    );
  }).toPass();
  await expect(page.getByRole("button", { name: "Reopen" })).toBeVisible();
  await page.goto(`/t/${slug}/messages`);
  await expect(page.getByText("No message here.")).toBeVisible();

  // The visitor asks to be forgotten: the message is erased for good.
  await page.goto(`/t/${slug}/messages?status=handled`);
  await page.getByRole("button", { name: "Erase" }).click();
  await expect(page.getByRole("status")).toContainText("Message erased");
  await expect(async () => {
    await page.goto(`/t/${slug}/messages?status=all`);
    await expect(page.getByText("No message here.")).toBeVisible();
  }).toPass();
});

test("a shopper signs up to the newsletter from the footer and confirms from the mail", async ({
  page,
  request,
  browser,
}) => {
  const owner = uniqueEmail("storefront-news");
  await registerWithEmail(page, owner);
  const { name, slug } = uniqueTenant("Storefront News");
  await createTenant(page, name);
  await app.start(request, await mintKey(page, slug, true), 0);

  // No storefront address yet: no link can be mailed, and the shopper is
  // told the newsletter is unavailable.
  // The browser's time zone rides along, for scheduled campaigns.
  const shopperContext = await browser.newContext({
    locale: "fr-FR",
    timezoneId: "America/New_York",
  });
  const shopper = await shopperContext.newPage();
  const email = uniqueEmail("abonne");
  await shopper.goto(app.url);
  const footer = shopper.locator(".sitefooter [data-newsletter-form]");
  await footer.getByLabel("Votre e-mail").fill(email);
  await footer.getByRole("button", { name: "S'inscrire" }).click();
  await expect(shopper).toHaveURL(
    `${app.url}/newsletter?error=newsletter_unavailable`,
  );
  await expect(shopper.getByRole("alert")).toContainText("indisponible");

  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Where shoppers find the shop").fill(app.url);
  await page.getByRole("button", { name: "Save address" }).click();
  await expect(page.getByRole("status")).toContainText(
    "Storefront address saved.",
  );

  // Signed up from the footer: the page says to look in the mailbox, and
  // the shop mails the link.
  await shopper.goto(app.url);
  await footer.getByLabel("Votre e-mail").fill(email.toUpperCase());
  await footer.getByRole("button", { name: "S'inscrire" }).click();
  await expect(shopper).toHaveURL(`${app.url}/newsletter?done=sent`);
  await expect(shopper.locator("[data-newsletter-done]")).toContainText(
    "cliquez sur le lien",
  );
  const mail = await mailTo(request, email, "Confirm your subscription to");
  expect(mail.subject).toBe(`Confirm your subscription to ${name}'s news`);
  // The mail's HTML escapes every "/" in an attribute as "&#x2f;".
  const link = (mail.html ?? "")
    .replaceAll("&#x2f;", "/")
    .match(/href="([^"]*\/newsletter\/confirm\/[^"]+)"/)?.[1];
  expect(link, "a confirmation link").toBeDefined();
  expect(link!.startsWith(`${app.url}/newsletter/confirm/`)).toBe(true);

  // Asking again at once mails nothing more.
  await shopper.goto(app.url);
  await footer.getByLabel("Votre e-mail").fill(email);
  await footer.getByRole("button", { name: "S'inscrire" }).click();
  await expect(shopper).toHaveURL(`${app.url}/newsletter?done=sent`);

  // Waiting for confirmation in the back office.
  const row = page.locator(`[data-subscriber="${email}"]`);
  await expect(async () => {
    await page.goto(`/t/${slug}/newsletter?status=pending`);
    await expect(row).toHaveAttribute("data-subscriber-status", "pending");
  }).toPass();
  await expect(row.locator("[data-subscriber-time-zone]")).toHaveText(
    "America/New_York",
  );
  await expect(row.locator("[data-subscriber-source]")).toHaveAttribute(
    "data-subscriber-source",
    "form",
  );
  await expect(row).toContainText("fr");

  // Opening the link asks for a click; a tampered one is refused.
  await shopper.goto(`${link!}x`);
  await shopper
    .getByRole("button", { name: "Confirmer mon inscription" })
    .click();
  await expect(shopper).toHaveURL(
    `${app.url}/newsletter?error=newsletter_invalid_link`,
  );
  await shopper.goto(link!);
  await shopper
    .getByRole("button", { name: "Confirmer mon inscription" })
    .click();
  await expect(shopper).toHaveURL(`${app.url}/newsletter?done=confirmed`);
  await expect(shopper.locator("[data-newsletter-done]")).toContainText(
    "C'est confirmé",
  );
  expect(
    await mailCountOf(request, email, "Confirm your subscription to"),
  ).toBe(1);

  await expect(async () => {
    await page.goto(`/t/${slug}/newsletter`);
    await expect(row).toHaveAttribute("data-subscriber-status", "confirmed");
  }).toPass();
  await expect(
    page.getByText("1 subscriber(s) get the shop's news."),
  ).toBeVisible();

  // A link that names nobody unsubscribes nobody.
  await shopper.goto(`${app.url}/newsletter/unsubscribe/nobody.abc`);
  await shopper.getByRole("button", { name: "Me désinscrire" }).click();
  await expect(shopper).toHaveURL(
    `${app.url}/newsletter?error=newsletter_invalid_link`,
  );

  // The shopper signs in: the account says they are subscribed, and they
  // leave the list from there, and come back without any link to follow.
  await shopper.goto(`${app.url}/account/sign-in`);
  const before = await mailCount(request, email);
  await shopper.getByLabel("E-mail", { exact: true }).fill(email);
  await shopper.getByRole("button", { name: "Recevoir un code" }).click();
  const code = await latestCode(request, email, before);
  await shopper.getByLabel("Code", { exact: true }).fill(code);
  await shopper.getByRole("button", { name: "Valider" }).click();
  await shopper.goto(`${app.url}/account`);
  const section = shopper.locator("[data-account-newsletter]");
  await expect(section).toHaveAttribute("data-account-newsletter", "on");
  await section
    .getByRole("button", { name: "Ne plus recevoir la newsletter" })
    .click();
  await expect(section).toHaveAttribute("data-account-newsletter", "off");
  await expect(async () => {
    await page.goto(`/t/${slug}/newsletter?status=unsubscribed`);
    await expect(row).toHaveAttribute("data-subscriber-status", "unsubscribed");
  }).toPass();
  await section.getByRole("button", { name: "Recevoir la newsletter" }).click();
  await expect(section).toHaveAttribute("data-account-newsletter", "on");
  expect(
    await mailCountOf(request, email, "Confirm your subscription to"),
  ).toBe(1);

  // The merchant erases them, as they asked: gone from every list.
  await expect(async () => {
    await page.goto(`/t/${slug}/newsletter`);
    await expect(row).toHaveAttribute("data-subscriber-status", "confirmed");
  }).toPass();
  await row.getByRole("button", { name: "Erase" }).click();
  await expect(page.getByRole("status")).toContainText("Subscriber erased");
  await expect(async () => {
    await page.goto(`/t/${slug}/newsletter?status=all`);
    await expect(page.getByText("No subscriber here.")).toBeVisible();
  }).toPass();
  await shopperContext.close();
});

/// Signs `email` up from the storefront footer and follows the mailed
/// link, in a browser of its own.
async function subscribeFromFooter(
  browser: Browser,
  request: APIRequestContext,
  email: string,
) {
  const context = await browser.newContext({ locale: "fr-FR" });
  const shopper = await context.newPage();
  await shopper.goto(app.url);
  const footer = shopper.locator(".sitefooter [data-newsletter-form]");
  await footer.getByLabel("Votre e-mail").fill(email);
  await footer.getByRole("button", { name: "S'inscrire" }).click();
  await expect(shopper).toHaveURL(`${app.url}/newsletter?done=sent`);
  const mail = await mailTo(request, email, "Confirm your subscription to");
  const link = (mail.html ?? "")
    .replaceAll("&#x2f;", "/")
    .match(/href="([^"]*\/newsletter\/confirm\/[^"]+)"/)![1];
  await shopper.goto(link);
  await shopper
    .getByRole("button", { name: "Confirmer mon inscription" })
    .click();
  await expect(shopper).toHaveURL(`${app.url}/newsletter?done=confirmed`);
  await context.close();
}

test("a merchant sends a campaign to the confirmed subscribers, who leave in one click", async ({
  page,
  request,
  browser,
}) => {
  const owner = uniqueEmail("storefront-campaign");
  await registerWithEmail(page, owner);
  const { name, slug } = uniqueTenant("Storefront Campaign");
  await createTenant(page, name);
  await app.start(request, await mintKey(page, slug, true), 0);
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Where shoppers find the shop").fill(app.url);
  await page.getByRole("button", { name: "Save address" }).click();
  await expect(page.getByRole("status")).toContainText(
    "Storefront address saved.",
  );
  const ada = uniqueEmail("ada");
  const grace = uniqueEmail("grace");
  await subscribeFromFooter(browser, request, ada);
  await subscribeFromFooter(browser, request, grace);
  await expect(async () => {
    await page.goto(`/t/${slug}/newsletter`);
    await expect(page.locator("[data-subscriber-count]")).toHaveText(
      "2 subscriber(s)",
    );
  }).toPass();

  // Written from the built-in start; one that does not link to the
  // unsubscribe page is refused.
  await page.getByRole("link", { name: "Campaigns" }).click();
  await expect(page.getByText("No campaign yet.")).toBeVisible();
  await page.getByRole("link", { name: "New campaign" }).click();
  const form = page.locator("[data-campaign-form]");
  await form.getByLabel("Subject").fill("Les nouveautés de {{ shop.name }}");
  await form
    .getByLabel("Body (HTML)")
    .fill("<p>Le panier douillet est arrivé.</p>");
  await form.getByRole("button", { name: "Save draft" }).click();
  await expect(page.locator("[data-template-error]")).toContainText(
    "unsubscribe_url",
  );
  await form
    .getByLabel("Body (HTML)")
    .fill(
      '<p>Le panier douillet est arrivé.</p><p><a href="{{ unsubscribe_url }}">Se désinscrire</a></p>',
    );
  await form.getByRole("button", { name: "Preview" }).click();
  await expect(page.locator("[data-preview-subject]")).toHaveText(
    `Les nouveautés de ${name}`,
  );
  // Marketing goes through the shop's own mail server only: without one,
  // neither the test nor the campaign goes out.
  await form.getByRole("button", { name: "Send me a test" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "shop's own mail server only",
  );
  await form.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByRole("status")).toContainText("Draft saved.");
  const campaignUrl = page.url().split("?")[0];
  await page.getByRole("link", { name: "Send…" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "shop's own mail server only",
  );
  await expect(
    page.getByRole("button", { name: "Send to 2 subscriber(s)" }),
  ).toHaveCount(0);
  await page.getByRole("link", { name: "Set up the mail server" }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/mail$`));
  const newsFrom = `news-${slug}@example.com`;
  await page.getByLabel("Server", { exact: true }).fill("localhost");
  await page.getByLabel("Port", { exact: true }).fill("1025");
  await page.getByLabel("Security").selectOption("none");
  await page.getByLabel("Send from address").fill(newsFrom);
  await page.getByRole("button", { name: "Test and save" }).click();
  await expect(page.getByRole("status")).toContainText("Test mail sent");

  await page.goto(campaignUrl);
  await form.getByRole("button", { name: "Send me a test" }).click();
  await expect(page.getByRole("status")).toContainText(
    `Test mail sent to ${owner}`,
  );
  const test = await mailTo(request, owner, "[Test] Les nouveautés de");
  expect(test.from[0].address).toBe(newsFrom);

  // Sent to the two confirmed addresses, once each, from the shop's server.
  await page.goto(campaignUrl);
  await page.getByRole("link", { name: "Send…" }).click();
  await expect(page.locator("[data-campaign-audience]")).toHaveAttribute(
    "data-campaign-audience",
    "2",
  );
  await page.getByRole("button", { name: "Send to 2 subscriber(s)" }).click();
  await expect(page.getByRole("status")).toContainText("Sending");
  const received = await mailTo(request, ada, "Les nouveautés de");
  expect(received.subject).toBe(`Les nouveautés de ${name}`);
  expect(received.from[0].address).toBe(newsFrom);
  await mailTo(request, grace, "Les nouveautés de");
  await expect(async () => {
    await page.reload();
    await expect(page.locator("dd [data-campaign-status]")).toHaveAttribute(
      "data-campaign-status",
      "sent",
    );
    await expect(page.locator("[data-campaign-delivered]")).toHaveText("2");
  }).toPass({ timeout: 20_000 });
  expect(await mailCountOf(request, ada, "Les nouveautés de")).toBe(1);
  // Sent: no more editing.
  await expect(page.locator("[data-campaign-form]")).toHaveCount(0);

  // The mail's own unsubscribe link, followed by a mail client's one-click
  // POST: Ada is off the list, Grace stays.
  const unsubscribe = (received.html ?? "")
    .replaceAll("&#x2f;", "/")
    .match(/href="([^"]*\/newsletter\/unsubscribe\/[^"]+)"/)?.[1];
  expect(unsubscribe, "an unsubscribe link").toBeDefined();
  const oneClick = await request.post(unsubscribe!, {
    form: { "List-Unsubscribe": "One-Click" },
    maxRedirects: 0,
  });
  expect(oneClick.status()).toBe(303);
  expect(oneClick.headers()["location"]).toContain("done=unsubscribed");
  await expect(async () => {
    await page.goto(`/t/${slug}/newsletter?status=unsubscribed`);
    await expect(page.locator(`[data-subscriber="${ada}"]`)).toBeVisible();
  }).toPass();
  await page.goto(`/t/${slug}/newsletter`);
  await expect(page.locator(`[data-subscriber="${grace}"]`)).toBeVisible();
  await expect(page.locator("[data-subscriber-count]")).toHaveText(
    "1 subscriber(s)",
  );
});

test("a shop without AliExpress asks for the place from the lists", async ({
  page,
  request,
}) => {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  await registerWithEmail(page, uniqueEmail("storefront-places"));
  const { name, slug } = uniqueTenant("Storefront Places");
  await createTenant(page, name);
  await publishProduct(page, slug, "Blue Mug", "12.50");
  await app.start(request, await mintKey(page, slug, true), 1);
  await page.goto(`${app.url}/p/blue-mug`);
  await addToCart(page);
  await page.goto(`${app.url}/checkout`);

  // The postal code first: it names the province, which is not asked for, and
  // what is picked is what the form sends.
  const province = page.getByLabel("Département", { exact: true });
  const city = page.getByLabel("Ville", { exact: true });
  await expect(province).toBeHidden();
  await expect(city).toBeDisabled();
  await expect(city.locator("option").first()).toHaveText(
    "Saisissez d'abord votre code postal",
  );
  const postalCode = page.getByLabel("Code postal", { exact: true });
  const sentCity = page.locator("input[name=city]");
  const typed = async (code: string) => {
    const answered = page.waitForResponse(/\/checkout\/places/);
    await postalCode.fill(code);
    await answered;
  };
  // As it is typed, without leaving the field.
  await typed("30100");
  await expect(province).toHaveValue("Gard");
  await expect(province).toBeHidden();
  await city.selectOption("Ales");
  await expect(sentCity).toHaveValue("Ales");

  // A code still being typed, or another in the same département, leaves the
  // city picked.
  await typed("301");
  await expect(province).toHaveValue("Gard");
  await expect(sentCity).toHaveValue("Ales");
  await typed("30000");
  await expect(sentCity).toHaveValue("Ales");

  // One in another département: its city is picked again, the old one not sent.
  await typed("75002");
  await expect(province).toHaveValue("Paris");
  await expect(city).toHaveValue("");
  await expect(sentCity).toHaveValue("");

  // A code naming no département: the shopper picks it, until one does.
  await typed("98000");
  await expect(province).toBeVisible();
  await expect(province).toHaveValue("");
  await province.selectOption("Var");
  await expect(city.locator("option[value=Toulon]")).toHaveCount(1);
  await typed("30100");
  await expect(province).toHaveValue("Gard");
  await expect(province).toBeHidden();
  await city.selectOption("Ales");

  // A town the list does not have is typed under its "Other".
  await city.selectOption("Other");
  await expect(page.getByLabel("Votre ville")).toBeVisible();

  expect(errors).toEqual([]);
});

test("a postal code overseas moves the order there, when the shop ships there", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("storefront-overseas"));
  const { name, slug } = uniqueTenant("Storefront Overseas");
  await createTenant(page, name);
  await publishProduct(page, slug, "Blue Mug", "12.50");
  await addShippingMethod(page, slug, "Métropole", "4,90", "France FR");
  await addShippingMethod(page, slug, "Outre-mer", "9,90", "Martinique MQ");
  await app.start(request, await mintKey(page, slug, true), 1);
  await page.goto(`${app.url}/p/blue-mug`);
  await addToCart(page);
  await page.goto(`${app.url}/checkout`);

  const country = page.getByLabel("Pays", { exact: true });
  const postalCode = page.getByLabel("Code postal", { exact: true });
  const shipping = page.locator("[data-checkout-shipping]");
  await expect(country).toHaveValue("FR");
  await expect(shipping).toHaveText("4,90€");

  // Martinique's postal code under France: delivered there, at its price.
  await postalCode.fill("97200");
  await postalCode.blur();
  await expect(country).toHaveValue("MQ");
  await expect(shipping).toHaveText("9,90€");

  // A postal code in France, and France chosen again: it stays.
  await postalCode.fill("75002");
  await postalCode.blur();
  await country.selectOption("FR");
  await expect(page.getByLabel("Département")).toHaveValue("Paris");
  await expect(country).toHaveValue("FR");
  await expect(shipping).toHaveText("4,90€");
});

test("an address that moved answers with a redirect for good", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("storefront-moved"));
  const { name, slug } = uniqueTenant("Storefront Moved");
  await createTenant(page, name);
  await createCategory(page, slug, "Mugs");
  const mug = await publishProduct(page, slug, "Blue Mug", "12.50", "Mugs");
  const old = await publishProduct(page, slug, "Old Mug", "9.90", "Mugs");
  const delivery = await publishPage(page, slug, "Livraison", "Sous 48 h.");
  await app.start(request, await mintKey(page, slug, false), 2);

  /// Where `path` answers from, without following: the status and the
  /// `Location`.
  const answer = async (path: string) => {
    const response = await request.get(`${app.url}${path}`, { maxRedirects: 0 });
    return [response.status(), response.headers()["location"] ?? null];
  };
  /// Waits for the storefront's copy to follow the back office.
  const leads = async (path: string, to: string | null) => {
    await expect(async () => {
      expect(await answer(path)).toEqual(to ? [308, to] : [404, null]);
    }).toPass({ timeout: 15_000 });
  };
  await leads("/p/tea-cup", null);
  expect((await answer("/p/blue-mug"))[0]).toBe(200);

  // A product renamed: its old address leads to the new one.
  await page.goto(mug);
  await page.getByLabel("Name", { exact: true }).fill("Tea Cup");
  await page.getByRole("button", { name: "Rename" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Tea Cup");
  await leads("/p/blue-mug", "/p/tea-cup");
  expect((await answer("/p/tea-cup"))[0]).toBe(200);
  // A browser lands on the product, which names its new address.
  await page.goto(`${app.url}/p/blue-mug`);
  await expect(page).toHaveURL(`${app.url}/p/tea-cup`);
  await expect(page.locator("link[rel=canonical]")).toHaveAttribute("href", `${app.url}/p/tea-cup`);

  // A product archived: its address leads to the category it was in.
  await page.goto(old);
  await page.getByRole("button", { name: "Archive" }).click();
  await leads("/p/old-mug", "/c/mugs");

  // A page moved to the address the shop serves it at.
  expect((await answer("/livraison"))[0]).toBe(404);
  await page.goto(delivery);
  await page.locator("#page-slug").fill("politique-de-livraison");
  await page.getByRole("button", { name: "Move", exact: true }).click();
  await leads("/livraison", "/politique-de-livraison");

  // The merchant's own list, for the addresses of another platform.
  await page.goto(`/t/${slug}/redirects`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Redirects");
  await page.getByLabel("Old path").fill("/products/blue-mug/");
  await page.getByLabel("Sends to").fill("/p/tea-cup");
  await page.getByRole("button", { name: "Add redirect" }).click();
  await expect(page.getByRole("status")).toContainText("Redirect added");
  const row = page.locator('[data-redirect="/products/blue-mug"]');
  await expect(row.locator("[data-redirect-to]")).toHaveText("/p/tea-cup");
  await leads("/products/blue-mug", "/p/tea-cup");
  await leads("/products/blue-mug/", "/p/tea-cup");
  // Checked: no chain, nothing twice, nowhere outside the shop.
  for (const [from, to, refusal] of [
    ["/products/blue-mug", "/p/other", "already redirects"],
    ["/older", "/products/blue-mug", "cannot lead to another redirect"],
    ["/elsewhere", "https://example.com/", "A redirect goes from an old path"],
  ]) {
    await page.getByLabel("Old path").fill(from);
    await page.getByLabel("Sends to").fill(to);
    await page.getByRole("button", { name: "Add redirect" }).click();
    await expect(page.getByRole("alert")).toContainText(refusal);
  }
  // An address nobody ever had stays unknown, and a posted form is never
  // sent on.
  expect(await answer("/products/other")).toEqual([404, null]);
  expect(await answer("/p/never-was")).toEqual([404, null]);
  const posted = await request.post(`${app.url}/products/blue-mug`, { maxRedirects: 0 });
  expect(posted.status()).not.toBe(308);

  // Deleted, the old path is unknown again.
  await row.getByRole("button", { name: "Delete the redirect from /products/blue-mug" }).click();
  await expect(page.getByRole("status")).toContainText("Redirect deleted.");
  await leads("/products/blue-mug", null);
});

test("a category opens on the products put first, then its guide and the questions it answers", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("storefront-hub"));
  const { name, slug } = uniqueTenant("Storefront Hub");
  await createTenant(page, name);
  await createCategory(page, slug, "Tapis");
  const category = page.url().split("?")[0];
  // Published in this order: the newest, the last, heads the listing.
  await publishProduct(page, slug, "Tapis bleu", "19.90", "Tapis");
  await publishProduct(page, slug, "Tapis rose", "24.90", "Tapis");
  await publishProduct(page, slug, "Tapis gris", "29.90", "Tapis");
  await publishProduct(page, slug, "Gamelle", "9.90");
  await app.start(request, await mintKey(page, slug, false), 4);

  const listed = async () =>
    page.locator("ul.catalog > li[data-product]").evaluateAll((cards) =>
      cards.map((card) => card.getAttribute("data-product")),
    );
  await page.goto(`${app.url}/c/tapis`);
  expect(await listed()).toEqual(["tapis-gris", "tapis-rose", "tapis-bleu"]);
  await expect(page.locator("[data-category-body]")).toHaveCount(0);
  await expect(page.locator("[data-category-questions]")).toHaveCount(0);

  // The merchant writes the guide and the questions, and puts two products
  // first: one of the category's, one filed elsewhere.
  await page.goto(category);
  await page
    .getByLabel("Buying guide, below the products")
    .fill("## Bien choisir son tapis\n\nSelon la **taille** du chat.\n\n<script>alert(1)</script>");
  await page.getByRole("button", { name: "Save description" }).click();
  await expect(page.getByRole("status")).toContainText("Description saved.");
  await page
    .getByLabel("Questions and answers")
    .fill("Quelle taille choisir ?\nMesurez votre chat.\nAjoutez 10 cm.\n\nLavable en machine ?\nOui, à 30°.");
  await page.getByRole("button", { name: "Save questions" }).click();
  await expect(page.getByRole("status")).toContainText("Questions saved.");
  await expect(page.locator("[data-category-questions]")).toHaveAttribute("data-category-questions", "2");
  await expect(page.getByLabel("Questions and answers")).toHaveValue(
    "Quelle taille choisir ?\nMesurez votre chat.\nAjoutez 10 cm.\n\nLavable en machine ?\nOui, à 30°.",
  );
  // A question without its answer is refused, as is a product nobody sells.
  await page.getByLabel("Questions and answers").fill("Pourquoi ?");
  await page.getByRole("button", { name: "Save questions" }).click();
  await expect(page.getByRole("alert")).toContainText("A category answers at most 20 questions");
  await page.getByLabel("Products shown first").fill("never-was");
  await page.getByRole("button", { name: "Save the first products" }).click();
  await expect(page.getByRole("alert")).toContainText("This product does not exist.");
  await page.getByLabel("Products shown first").fill("tapis-bleu\n/p/gamelle\n");
  await page.getByRole("button", { name: "Save the first products" }).click();
  await expect(page.getByRole("status")).toContainText("First products saved.");
  await expect(page.locator("[data-category-pinned]")).toHaveText("tapis-bleu, gamelle");

  // The page opens on the one it lists; the one filed elsewhere is not
  // shown here.
  await expect(async () => {
    await page.goto(`${app.url}/c/tapis`);
    expect(await listed()).toEqual(["tapis-bleu", "tapis-gris", "tapis-rose"]);
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("[data-results]")).toHaveAttribute("data-results", "3");
  // Sorted another way, the order is the shopper's.
  await page.goto(`${app.url}/c/tapis?sort=price_desc`);
  expect(await listed()).toEqual(["tapis-gris", "tapis-rose", "tapis-bleu"]);
  await expect(page.locator("[data-results]")).toHaveAttribute("data-results", "3");

  // Below the products: the guide as the merchant wrote it, then each
  // question, opening on its answer.
  await page.goto(`${app.url}/c/tapis`);
  const guide = page.locator("[data-category-body]");
  await expect(guide.locator("h2")).toHaveText("Bien choisir son tapis");
  await expect(guide.locator("strong")).toHaveText("taille");
  await expect(guide.locator("script")).toHaveCount(0);
  await expect(guide).toContainText("<script>alert(1)</script>");
  const questions = page.locator("[data-category-questions]");
  await expect(questions.getByRole("heading", { level: 2 })).toHaveText("Questions fréquentes");
  await expect(questions.locator("summary")).toHaveText(["Quelle taille choisir ?", "Lavable en machine ?"]);
  const answer = questions.locator("details").first().locator("p");
  await expect(answer).toBeHidden();
  await questions.locator("summary").first().click();
  await expect(answer).toHaveText("Mesurez votre chat.\nAjoutez 10 cm.");
  // No FAQ markup: search engines stopped showing it.
  const data = await page.locator('script[type="application/ld+json"]').allTextContents();
  expect(data.join("")).not.toContain("FAQPage");
});

test("a listing is narrowed by the options the merchant marks as filters", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("storefront-filters"));
  const { name, slug } = uniqueTenant("Storefront Filters");
  await createTenant(page, name);
  await createCategory(page, slug, "Chat");
  await publishVariedProduct(page, slug, "Tapis", "Taille", [["S", "19.90"], ["M", "24.90"]], "Chat");
  await publishVariedProduct(page, slug, "Coussin", "Taille", [["M", "14.90"]], "Chat");
  await publishVariedProduct(page, slug, "Collier", "Couleur", [["Rose", "9.90"]], "Chat");
  await app.start(request, await mintKey(page, slug, false), 3);

  // Until the merchant chooses, the storefront offers the colour alone.
  const filters = page.locator("[data-filters]");
  await page.goto(`${app.url}/c/chat`);
  await expect(filters.locator("[data-filter-group]")).toHaveCount(1);
  await expect(filters.locator("[data-filter=couleur]")).toHaveText(["Rose (1)"]);

  // The merchant marks the size, then the colour.
  await page.goto(`/t/${slug}/categories`);
  await expect(page.locator("[data-listing-filters]")).toHaveAttribute("data-listing-filters", "unset");
  await page.getByLabel("Options shoppers filter by").fill("Taille\nCouleur\ntaille\n");
  await page.getByRole("button", { name: "Save filters" }).click();
  await expect(page.getByRole("status")).toContainText("Filters saved.");
  await expect(page.getByLabel("Options shoppers filter by")).toHaveValue("Taille\nCouleur");
  await expect(page.locator("[data-listing-filters]")).toHaveAttribute("data-listing-filters", "set");

  // The storefront offers both, in that order, with what each value counts.
  await expect(async () => {
    await page.goto(`${app.url}/c/chat`);
    await expect(filters.locator("[data-filter-group]")).toHaveCount(2, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await filters.locator("summary").click();
  await expect(filters.locator("h3")).toHaveText([
    "Filtrer par tarif",
    "Filtrer par taille",
    "Filtrer par couleur",
  ]);
  await expect(filters.locator("[data-filter=taille]")).toHaveText(["M (2)", "S (1)"]);
  // Narrowed, the products are those of the plain listing again: its links
  // are not followed, and the page is kept out of search engines.
  const medium = filters.locator("[data-filter=taille][data-filter-value=m]");
  await expect(medium).toHaveAttribute("rel", "nofollow");
  await medium.click();
  await expect(page).toHaveURL(`${app.url}/c/chat?taille=m`);
  await expect(page.locator("[data-results]")).toHaveAttribute("data-results", "2");
  await expect(page.locator("ul.catalog > li[data-product]")).toHaveCount(2);
  await expect(medium).toHaveAttribute("aria-current", "true");
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute("content", "noindex");
  await expect(page.locator("link[rel=canonical]")).toHaveAttribute("href", `${app.url}/c/chat`);
  // The other filter counts what is left; a sort keeps the filter.
  await expect(filters.locator("[data-filter=couleur]")).toHaveCount(0);
  await page.locator("[data-sort]").selectOption("price_asc");
  await page.getByRole("button", { name: "Trier" }).click();
  await expect(page).toHaveURL(/sort=price_asc/);
  await expect(page).toHaveURL(/taille=m/);
  await expect(page.locator("ul.catalog > li[data-product]").first()).toHaveAttribute("data-product", "coussin");
  // The whole catalogue offers them too.
  await page.goto(`${app.url}/boutique?taille=s`);
  await expect(page.locator("ul.catalog > li[data-product]")).toHaveCount(1);
  // Crawlers are told to stay out of each.
  const rules = await (await request.get(`${app.url}/robots.txt`)).text();
  expect(rules).toContain("Disallow: /*?*taille=\n");
  expect(rules).toContain("Disallow: /*?*couleur=\n");

  // None at all: the price alone, and an address naming one shows everything.
  await page.goto(`/t/${slug}/categories`);
  await page.getByLabel("Options shoppers filter by").fill("");
  await page.getByRole("button", { name: "Save filters" }).click();
  await expect(page.getByRole("status")).toContainText("Filters saved.");
  await expect(async () => {
    await page.goto(`${app.url}/c/chat?taille=m`);
    await expect(filters.locator("[data-filter-group]")).toHaveCount(0, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(page.locator("[data-results]")).toHaveAttribute("data-results", "3");
});

test("the blog lists the merchant's articles, each with its contents and the products it shows", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("storefront-blog"));
  const { name, slug } = uniqueTenant("Storefront Blog");
  await createTenant(page, name);
  await createCategory(page, slug, "Tapis");
  await publishProduct(page, slug, "Tapis bleu", "19.90", "Tapis");
  await app.start(request, await mintKey(page, slug, false), 1);

  // Nothing published: the blog answers, kept out of search engines, the
  // footer does not link to it and the sitemap does not list it.
  await page.goto(`${app.url}/blog`);
  await expect(page.locator("[data-blog-empty]")).toBeVisible();
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute("content", "noindex");
  await expect(page.locator("[data-blog-link]")).toHaveCount(0);
  expect(await (await request.get(`${app.url}/sitemap-pages.xml`)).text()).not.toContain("/blog");

  // The merchant writes an article: a summary, two parts, a product and a
  // category of the shop in between.
  await page.goto(`/t/${slug}/articles`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Blog");
  await expect(page.getByText("No article yet.")).toBeVisible();
  await page.getByRole("link", { name: "New article" }).click();
  await page.locator("#article-title").fill("Bien choisir son tapis");
  await page.locator("#article-summary").fill("Les tailles et les matières, en deux minutes.");
  await page
    .locator("#article-body")
    .fill(
      "Un tapis se choisit **à la taille** du chat.\n\n## Les tailles\n\nDu S au XL.\n\n[[product:tapis-bleu]]\n\n" +
        "## Les matières\n\nLe coton respire.\n\n[[category:tapis]]\n\n[[product:never-was]]\n\n<script>alert(1)</script>",
    );
  await page.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByRole("status")).toContainText("Draft saved.");
  await expect(page.locator("#article-slug")).toHaveValue("bien-choisir-son-tapis");
  const article = page.url().split("?")[0];
  // The preview names what the storefront will show there.
  const preview = page.frameLocator("[data-article-preview]");
  await expect(preview.locator("h1")).toHaveText("Bien choisir son tapis");
  await expect(preview.locator(".lead")).toHaveText("Les tailles et les matières, en deux minutes.");
  await expect(preview.locator("h2#s-les-tailles")).toHaveText("Les tailles");
  await expect(preview.locator(".shown").first()).toHaveText("Product: tapis-bleu");
  await expect(preview.locator("script")).toHaveCount(0);

  // A picture, made the cover.
  await page
    .getByLabel("Add a picture")
    .setInputFiles({ name: "tapis.png", mimeType: "image/png", buffer: PNG });
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added");
  await page.getByRole("button", { name: "Use as cover" }).click();
  await expect(page.getByRole("status")).toContainText("Cover set.");
  await expect(page.locator("[data-article-cover]")).toBeVisible();

  // A draft is not on the blog.
  expect((await request.get(`${app.url}/blog/bien-choisir-son-tapis`)).status()).toBe(404);
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.getByRole("status")).toContainText("Published");
  await expect(page.locator("[data-article-status]")).toHaveAttribute("data-article-status", "Published");
  const today = new Date().toISOString().slice(0, 10);
  await expect(page.locator("#article-date")).toHaveValue(today);
  // A day ahead is refused.
  const ahead = new Date(Date.now() + 2 * 86_400_000).toISOString().slice(0, 10);
  await page.locator("#article-date").evaluate((input: HTMLInputElement, day) => {
    input.removeAttribute("max");
    input.value = day;
  }, ahead);
  await page.getByRole("button", { name: "Change the date" }).click();
  await expect(page.getByRole("alert")).toContainText("An article is dated today or before.");

  // The back office lists it, a moment behind.
  await expect(async () => {
    await page.goto(`/t/${slug}/articles`);
    await expect(page.locator("[data-article=bien-choisir-son-tapis]")).toHaveAttribute(
      "data-article-status",
      "Published",
      { timeout: 1000 },
    );
  }).toPass({ timeout: 15_000 });

  // The storefront lists it: its title, its day, its summary and its cover.
  await expect(async () => {
    await page.goto(`${app.url}/blog`);
    await expect(page.locator("[data-article=bien-choisir-son-tapis]")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  const card = page.locator("[data-article=bien-choisir-son-tapis]");
  await expect(card.locator(".post-summary")).toHaveText("Les tailles et les matières, en deux minutes.");
  await expect(card.locator("time")).toHaveAttribute("datetime", today);
  await expect(card.locator("img[data-image]")).toBeVisible();
  await expect(page.locator('meta[name="robots"]')).toHaveCount(0);
  await expect(page.locator("[data-blog-link]")).toHaveAttribute("href", "/blog");

  // The article: its contents lead to its parts, and it shows the product
  // and the category's products as the shop's own cards.
  await card.getByRole("link", { name: "Bien choisir son tapis" }).click();
  await expect(page).toHaveURL(`${app.url}/blog/bien-choisir-son-tapis`);
  const shown = page.locator("article[data-article]");
  await expect(shown.locator("h1")).toHaveText("Bien choisir son tapis");
  await expect(shown.locator(".post-lead")).toHaveText("Les tailles et les matières, en deux minutes.");
  await expect(shown.locator("img.post-cover")).toBeVisible();
  await expect(shown.locator("strong")).toHaveText("à la taille");
  await expect(shown.locator("script")).toHaveCount(0);
  await expect(shown).toContainText("<script>alert(1)</script>");
  const contents = shown.locator("[data-toc] a");
  await expect(contents).toHaveText(["Les tailles", "Les matières"]);
  await contents.nth(1).click();
  await expect(page).toHaveURL(`${app.url}/blog/bien-choisir-son-tapis#s-les-matieres`);
  await expect(shown.locator("h2#s-les-matieres")).toBeInViewport();
  // The product it names, then the category's; the product nobody sells
  // leaves nothing.
  const blocks = shown.locator("[data-article-products]");
  await expect(blocks).toHaveCount(2);
  await expect(blocks.nth(0).locator("[data-product=tapis-bleu] [data-price]")).toHaveText("19,90€");
  await expect(blocks.nth(0).locator("[data-article-category]")).toHaveCount(0);
  await expect(blocks.nth(1).locator("[data-article-category]")).toHaveAttribute("href", "/c/tapis");
  await expect(blocks.nth(1).locator("[data-product=tapis-bleu]")).toBeVisible();
  await expect(shown).not.toContainText("never-was");

  // What search engines and a shared link are told.
  await expect(page).toHaveTitle("Bien choisir son tapis – Ma boutique");
  await expect(page.locator('meta[name="description"]')).toHaveAttribute(
    "content",
    "Les tailles et les matières, en deux minutes.",
  );
  await expect(page.locator("link[rel=canonical]")).toHaveAttribute(
    "href",
    `${app.url}/blog/bien-choisir-son-tapis`,
  );
  await expect(page.locator('meta[property="og:type"]')).toHaveAttribute("content", "article");
  await expect(page.locator('meta[property="og:image"]')).toHaveAttribute("content", /\/1200\/.+\.webp$/);
  const data = (await page.locator('script[type="application/ld+json"]').allTextContents()).map((text) =>
    JSON.parse(text),
  );
  const posting = data.find((ld) => ld["@type"] === "BlogPosting");
  expect(posting.headline).toBe("Bien choisir son tapis");
  expect(posting.datePublished.slice(0, 10)).toBe(today);
  expect(posting.mainEntityOfPage).toBe(`${app.url}/blog/bien-choisir-son-tapis`);
  expect(posting.image).toHaveLength(1);
  expect(posting.publisher.name).toBe("Ma boutique");
  const trail = data.find((ld) => ld["@type"] === "BreadcrumbList");
  expect(trail.itemListElement.map((item: { name: string }) => item.name)).toEqual([
    "Accueil",
    "Blog",
    "Bien choisir son tapis",
  ]);
  const listed = await (await request.get(`${app.url}/sitemap-pages.xml`)).text();
  expect(listed).toContain(`<loc>${app.url}/blog</loc>`);
  expect(listed).toContain(`<loc>${app.url}/blog/bien-choisir-son-tapis</loc>`);

  // Moved to another address: the old one leads there for good.
  await page.goto(article);
  await page.locator("#article-slug").fill("choisir-un-tapis");
  await page.getByRole("button", { name: "Move", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Address changed.");
  await expect(async () => {
    const moved = await request.get(`${app.url}/blog/bien-choisir-son-tapis`, { maxRedirects: 0 });
    expect([moved.status(), moved.headers()["location"]]).toEqual([308, "/blog/choisir-un-tapis"]);
  }).toPass({ timeout: 15_000 });

  // Back to a draft: off the blog.
  await page.getByRole("button", { name: "Unpublish" }).click();
  await expect(page.getByRole("status")).toContainText("Unpublished");
  await expect(async () => {
    expect((await request.get(`${app.url}/blog/choisir-un-tapis`)).status()).toBe(404);
  }).toPass({ timeout: 15_000 });
  await page.goto(`${app.url}/blog`);
  await expect(page.locator("[data-blog-empty]")).toBeVisible();
});

test("a visitor answers the cookie banner, and Google's consent mode follows", async ({
  page,
  request,
}) => {
  await registerWithEmail(page, uniqueEmail("storefront-consent"));
  const { name, slug } = uniqueTenant("Storefront Consent");
  await createTenant(page, name);
  await app.start(request, await mintKey(page, slug), 0);

  /// What the page told Google's consent mode, in order: `default` then,
  /// once answered, `update`.
  const told = () =>
    page.evaluate(() =>
      ((window as unknown as { dataLayer: unknown[][] }).dataLayer ?? [])
        .map((entry) => Array.from(entry))
        .filter((entry) => entry[0] === "consent")
        .map((entry) => [entry[1], entry[2]]),
    );
  const all = (value: string) => ({
    ad_storage: value,
    ad_user_data: value,
    ad_personalization: value,
    analytics_storage: value,
  });
  const kept = async () =>
    (await page.context().cookies(app.url)).find((cookie) => cookie.name === "consent");

  // Nothing is allowed before an answer, and the banner asks on every page.
  await page.goto(`${app.url}/boutique?utm_source=lettre`);
  const banner = page.locator("[data-consent]");
  await expect(banner).toBeVisible();
  expect(await told()).toEqual([["default", all("denied")]]);
  expect(await kept()).toBeUndefined();

  // Refusing takes one click and comes back to the same address.
  await banner.getByRole("button", { name: "Tout refuser" }).click();
  await expect(page).toHaveURL(`${app.url}/boutique?utm_source=lettre`);
  await expect(banner).toHaveCount(0);
  expect((await kept())?.value).toBe("none");
  expect(await told()).toEqual([
    ["default", all("denied")],
    ["update", all("denied")],
  ]);
  // The answer is kept six months, then the banner asks again.
  const days = ((await kept())!.expires - Date.now() / 1000) / 86_400;
  expect(days).toBeGreaterThan(179);
  expect(days).toBeLessThan(181);

  // The footer leads to the choices, where the answer changes: one purpose
  // allowed, the other not.
  await page.getByRole("link", { name: "Gestion des cookies" }).click();
  await expect(page).toHaveURL(`${app.url}/cookies`);
  await expect(page.locator("meta[name=robots][content=noindex]")).toHaveCount(1);
  const choices = page.locator("[data-cookies]");
  await expect(choices).toHaveAttribute("data-cookies", "none");
  await expect(choices.getByRole("checkbox", { name: /Mesure d'audience/ })).not.toBeChecked();
  await choices.getByRole("checkbox", { name: /Mesure d'audience/ }).check();
  await choices.getByRole("button", { name: "Enregistrer mes choix" }).click();
  await expect(page.getByRole("status")).toHaveText("Votre choix est enregistré.");
  await expect(choices).toHaveAttribute("data-cookies", "analytics");
  await expect(choices.getByRole("checkbox", { name: /Mesure d'audience/ })).toBeChecked();
  await expect(choices.getByRole("checkbox", { name: /Publicité/ })).not.toBeChecked();
  expect(await told()).toEqual([
    ["default", all("denied")],
    ["update", { ...all("denied"), analytics_storage: "granted" }],
  ]);

  // Accepting everything grants the four signals on the pages that follow.
  await choices.getByRole("button", { name: "Tout accepter" }).click();
  await expect(choices).toHaveAttribute("data-cookies", "analytics.ads");
  await page.goto(`${app.url}/`);
  await expect(banner).toHaveCount(0);
  expect(await told()).toEqual([
    ["default", all("denied")],
    ["update", all("granted")],
  ]);

  // A browser without the answer is asked again, and can accept from the
  // banner; a form that names another site sends nobody there.
  await page.context().clearCookies({ name: "consent" });
  await page.goto(`${app.url}/boutique`);
  await banner.getByRole("button", { name: "Tout accepter" }).click();
  await expect(page).toHaveURL(`${app.url}/boutique`);
  expect((await kept())?.value).toBe("analytics.ads");
  const elsewhere = await request.post(`${app.url}/cookies`, {
    form: { answer: "all", from: "//example.com/" },
    maxRedirects: 0,
  });
  expect(elsewhere.status()).toBe(303);
  expect(elsewhere.headers()["location"]).toBe("/");
});

test("the storefront stops cleanly when told to", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("storefront-stop"));
  const { name, slug } = uniqueTenant("Storefront Stop");
  await createTenant(page, name);
  await app.start(request, await mintKey(page, slug, true), 0);

  // SIGTERM, as a host sends it: a clean exit, well before the drain and the
  // store's limits (10 s and 5 s) with nothing under way, and the port free.
  const asked = Date.now();
  expect(await app.terminate()).toBe(0);
  expect(Date.now() - asked).toBeLessThan(10_000);
  await expect(request.get(`${app.url}/store/status`)).rejects.toThrow();
});
