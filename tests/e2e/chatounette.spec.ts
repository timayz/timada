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
  ExampleApp,
  PNG,
  mintKey,
  publishProduct,
  publishVariedProduct,
} from "./storefront";
import { createTenant, uniqueTenant } from "./tenants";

// The Chatounette clone: the second example app, in French and in the
// layout of chatounette.com, booted on a small catalogue a merchant made in
// the dashboard (the real shop's catalogue is seeded from seeds/ outside
// the browser suite). It walks the home, the category bar, a category, a
// product, the cart drawer, the search and a shop page.

const app = new ExampleApp(
  "chatounette",
  Number(process.env.E2E_CHATOUNETTE_PORT ?? 3457),
);

test.afterEach(() => app.stop());

/// Picks the option of a `<select>` whose text contains `text`.
async function selectByText(page: Page, label: string, text: string) {
  const select = page.getByLabel(label);
  const value = await select
    .locator("option", { hasText: text })
    .first()
    .getAttribute("value");
  await select.selectOption(value ?? "");
}

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

/// A picture on the product whose dashboard page is open.
async function addPicture(page: Page, url: string, alt: string) {
  await page.goto(url);
  await page
    .getByLabel("Add a picture")
    .setInputFiles({ name: "photo.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill(alt);
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
}

test("the Chatounette clone shows the catalogue as a French cat shop", async ({
  page,
  request,
  browser,
}) => {
  const owner = uniqueEmail("chatounette");
  await registerWithEmail(page, owner);
  const { name, slug } = uniqueTenant("Chat Clone");
  await createTenant(page, name);

  await createCategory(page, slug, "Couchage");
  // The longer text the shop shows below a category's products.
  await page
    .getByLabel("Text below the products")
    .fill("Comment choisir un couchage pour chat ?");
  await page.getByRole("button", { name: "Save description" }).click();
  await expect(page.getByRole("status")).toContainText("Description saved.");
  await createCategory(page, slug, "Tapis", "Couchage");
  const mat = await publishProduct(
    page,
    slug,
    "Tapis fraîcheur",
    "19.90",
    "Tapis",
  );
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
  await expect(page.getByRole("heading", { level: 1 })).toContainText(
    "l'endroit parfait pour les accessoires",
  );
  await expect(page.locator(".topbar")).toContainText("MONSTER10");
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
  await expect(page.locator(".sitefooter")).toContainText(
    "Conditions générales de vente",
  );

  // A category: the breadcrumb, the sub-categories, the count and the
  // shop's sort labels.
  await page.goto(`${app.url}/c/couchage`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Couchage");
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
  await expect(filters.locator("[data-colour]")).toHaveText([
    "Bleu (1)",
    "Rose (1)",
  ]);
  await filters.locator("summary").click();
  await filters.locator("[data-colour=rose]").click();
  await expect(page).toHaveURL(/couleur=rose/);
  await expect(page.locator("[data-results]")).toContainText(
    "1 résultat affiché",
  );
  await expect(page.locator("ul.catalog [data-product]")).toHaveText(
    /Collier étoilé/,
  );
  await expect(filters).toHaveJSProperty("open", true);
  await expect(filters.locator("[data-colour=rose]")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(filters.locator("[data-colour=bleu]")).toHaveText("Bleu (1)");
  await page.locator("[data-sort]").selectOption("price_asc");
  await page.getByRole("button", { name: "Trier" }).click();
  await expect(page).toHaveURL(/sort=price_asc/);
  await expect(page).toHaveURL(/couleur=rose/);
  await filters.locator("[data-colour=rose]").click();
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
  await expect(filters.locator("[data-colour]")).toHaveText([
    "Bleu (1)",
    "Rose (1)",
  ]);
  await filters.locator("[data-price-band='70-100']").click();
  await expect(page.locator("[data-filters-empty]")).toContainText(
    "Aucun produit ne correspond à ces filtres.",
  );
  await expect(filters.locator("[data-colour]")).toHaveCount(0);
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
    page.locator("[data-filters] [data-colour=bleu]"),
  ).toHaveAttribute("aria-current", "true");
  await page.goto(`${app.url}/boutique?q=collier&couleur=gris`);
  await expect(page.locator("[data-filters-empty]")).toBeVisible();

  // A product: the French page, then into the cart from it, and from a card.
  await page.goto(`${app.url}/p/tapis-fraicheur`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Tapis fraîcheur",
  );
  await expect(page.locator("[data-crumbs]")).toContainText(
    "Accueil / Couchage / Tapis / Tapis fraîcheur",
  );
  await expect(page.locator(".buy [data-price]")).toHaveText("19,90€");
  await expect(page.locator("[data-reassure]")).toContainText(
    "Livraison gratuite dès 65€ d'achats",
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

  await page.goto(`${app.url}/pages/politique-de-livraison`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Politique de livraison",
  );
  await expect(
    page.locator("[data-page=politique-de-livraison]"),
  ).toContainText("65 €");
  expect((await page.goto(`${app.url}/pages/nothing-here`))?.status()).toBe(
    404,
  );
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
    "Support client : contact@chatounette.localhost",
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

test("a shopper signs up to the Chatounette newsletter from the footer and confirms from the mail", async ({
  page,
  request,
  browser,
}) => {
  const owner = uniqueEmail("chatounette-news");
  await registerWithEmail(page, owner);
  const { name, slug } = uniqueTenant("Chat News");
  await createTenant(page, name);
  await app.start(request, await mintKey(page, slug, true), 0);

  // No storefront address yet: no link can be mailed, and the shopper is
  // told the newsletter is unavailable.
  const shopperContext = await browser.newContext({ locale: "fr-FR" });
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

/// Signs `email` up from the Chatounette footer and follows the mailed
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
  const owner = uniqueEmail("chatounette-campaign");
  await registerWithEmail(page, owner);
  const { name, slug } = uniqueTenant("Chat Campaign");
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
