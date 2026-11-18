import { expect, test } from "@playwright/test";
import { registerWithEmail, uniqueEmail } from "./email";
import { productTab, selectByText } from "./forms";
import { invite, inviteLink } from "./invitations";
import { fileUnder, publishProduct } from "./storefront";
import { createTenant, uniqueTenant } from "./tenants";

// The catalog: an owner creates a product under /t/{slug}/products, edits it
// on its page, uploads its pictures, publishes, archives and deletes it; the
// list (a read model, a moment behind) follows and shows the published
// products unless asked otherwise; a member whose role lacks `products:read`
// cannot open the pages.

/// A 1x1 PNG, as the bytes a browser posts when a file is chosen.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

test("an owner creates, edits, publishes, archives and deletes a product", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Catalog Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Products", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/products$`));
  await expect(page.getByText("No published product yet.")).toBeVisible();

  // Create: lands on the product page, a draft without a price.
  await page.getByLabel("Name").fill("Blue Mug");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/products/[0-9A-Z]{26}\\?`));
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "draft");
  await expect(page.getByLabel("Price")).toHaveValue("");
  const productUrl = page.url().split("?")[0];

  // Publishing needs a price; a bad amount is refused.
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page).toHaveURL(/error=product_not_priced/);
  await page.getByLabel("Price").fill("abc");
  await page.getByRole("button", { name: "Set price" }).click();
  await expect(page).toHaveURL(/error=invalid_amount/);

  // Edit everything, then publish.
  await page.getByLabel("Price").fill("12.50");
  await page.getByLabel("Currency").selectOption("EUR");
  await page.getByRole("button", { name: "Set price" }).click();
  await expect(page.getByRole("status")).toContainText("Price set.");
  await expect(page.getByLabel("Price")).toHaveValue("12.50");
  await page.getByLabel("Description", { exact: true }).fill("Holds 50 cl of anything hot.");
  await page.getByRole("button", { name: "Save description" }).click();
  await expect(page.getByLabel("Description", { exact: true })).toHaveValue("Holds 50 cl of anything hot.");
  await page.getByLabel("Name", { exact: true }).fill("Big Blue Mug");
  await page.getByRole("button", { name: "Rename" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Big Blue Mug");
  await expect(page.getByText("big-blue-mug", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.getByRole("status")).toContainText("Product published.");
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");

  // The list comes from the read model, a moment behind.
  const row = page.locator("[data-product=big-blue-mug]");
  await expect(async () => {
    await page.goto(`/t/${slug}/products`);
    await expect(row).toHaveAttribute("data-product-status", "published", { timeout: 1000 });
    await expect(row).toContainText("Big Blue Mug", { timeout: 1000 });
    await expect(row).toContainText("12.50 €", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.getByLabel("Search").fill("nothing-like-it");
  await page.getByRole("button", { name: "Search" }).click();
  await expect(page.getByText("No product matches.")).toBeVisible();

  // Archive: no more edits.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Archive" }).click();
  await expect(page.getByRole("status")).toContainText("Product archived.");
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "archived");
  await expect(page.getByRole("button", { name: "Rename" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Archive" })).toHaveCount(0);
  // With no forms left to read them in, the overview says what it holds.
  await expect(page.locator("[data-product-price]")).toContainText("12.50 €");
  await expect(page.locator("[data-product-description]")).toContainText("Holds 50 cl");
  await expect(page.getByText("big-blue-mug", { exact: true })).toBeVisible();

  // The list shows the published products unless asked for another status.
  await expect(async () => {
    await page.goto(`/t/${slug}/products`);
    await expect(page.getByText("No published product yet.")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await selectByText(page, "Status", "Archived");
  await page.getByRole("button", { name: "Search" }).click();
  await expect(page).toHaveURL(/status=archived/);
  await expect(row).toHaveAttribute("data-product-status", "archived");

  // Delete: nobody ordered it, so it goes for good.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Delete product" }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/products\\?`));
  await expect(page.getByRole("status")).toContainText("Product deleted");
  expect((await page.request.get(productUrl)).status()).toBe(404);
  await expect(async () => {
    await page.goto(`/t/${slug}/products?status=all`);
    await expect(page.getByText("No product yet.")).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 15_000 });
});

test("the products pages need products:read", async ({ browser, request }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await registerWithEmail(owner, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Locked Catalog");
  await createTenant(owner, name);

  // A "viewer" role with no permission at all.
  await owner.getByRole("link", { name: "Roles", exact: true }).click();
  const define = owner.locator("form", { has: owner.getByRole("button", { name: "Define role" }) });
  await define.getByLabel("Role name").fill("viewer");
  await define.getByLabel("Label").fill("Viewer");
  await define.getByRole("button", { name: "Define role" }).click();
  await expect(owner.getByRole("status")).toContainText("Role defined.");
  const viewerEmail = uniqueEmail("viewer");
  await owner.getByRole("link", { name: "Members", exact: true }).click();
  await invite(owner, viewerEmail, "Viewer");
  const link = await inviteLink(request, viewerEmail);

  const viewerContext = await browser.newContext();
  const viewer = await viewerContext.newPage();
  await registerWithEmail(viewer, viewerEmail);
  await viewer.goto(link);
  await viewer.getByRole("button", { name: "Accept the invitation" }).click();
  await expect(viewer).toHaveURL(new RegExp(`/t/${slug}`));
  await expect(viewer.getByRole("link", { name: "Products", exact: true })).toHaveCount(0);
  expect((await viewer.goto(`/t/${slug}/products`))?.status()).toBe(403);

  await ownerContext.close();
  await viewerContext.close();
});

test("an owner uploads, describes, reorders and removes a product's pictures", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Picture Shop");
  await createTenant(page, name);

  await page.getByRole("link", { name: "Products", exact: true }).click();
  await page.getByLabel("Name").fill("Blue Mug");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const productUrl = page.url().split("?")[0];
  await productTab(page, "Pictures");
  await expect(page.getByText("No picture yet.")).toBeVisible();
  // The other tabs' forms are not on this one.
  await expect(page.getByRole("button", { name: "Rename" })).toHaveCount(0);

  // Anything that is not a picture is refused, whatever it is called.
  await page.getByLabel("Add a picture").setInputFiles({
    name: "mug.png",
    mimeType: "image/png",
    buffer: Buffer.from("this is not a picture at all"),
  });
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page).toHaveURL(/error=unsupported_image/);
  await expect(page).toHaveURL(/tab=pictures/);
  await expect(page.getByRole("alert")).toContainText("JPEG, PNG, WebP or GIF");

  // A real one is taken, shown, and served from the picture address.
  await page.getByLabel("Add a picture").setInputFiles({ name: "front.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill("A blue mug, seen from the front");
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
  // An action comes back to the tab it was sent from.
  await expect(page).toHaveURL(/tab=pictures/);
  await expect(page.getByRole("link", { name: "Pictures 1" })).toHaveAttribute("aria-current", "page");
  const first = page.locator("[data-image]").first();
  await expect(first.locator("img")).toHaveAttribute("alt", "A blue mug, seen from the front");
  await expect(first.locator("[data-image-first]")).toBeVisible();
  // What is served is a rendition: WebP, in the listing size.
  const src = await first.locator("img").getAttribute("src");
  expect(src).toMatch(/\/media\/[0-9A-Z]{26}\/400\/[0-9A-Z]{26}\.webp$/);
  const served = await request.get(src!);
  expect(served.status()).toBe(200);
  expect(served.headers()["content-type"]).toContain("image/webp");
  expect(served.headers()["cache-control"]).toContain("immutable");
  // And the page size next to it.
  const large = await request.get(src!.replace("/400/", "/1200/"));
  expect(large.status()).toBe(200);
  expect(large.headers()["content-type"]).toContain("image/webp");

  // A second one goes last, and the two can swap.
  await page.getByLabel("Add a picture").setInputFiles({ name: "side.png", mimeType: "image/png", buffer: PNG });
  await page.getByLabel("What it shows (optional)").fill("The same mug from the side");
  await page.getByRole("button", { name: "Upload" }).click();
  await expect(page.getByRole("status")).toContainText("Picture added.");
  await expect(page.locator("[data-image]")).toHaveCount(2);
  const second = page.locator("[data-image]").nth(1);
  await expect(second.locator("img")).toHaveAttribute("alt", "The same mug from the side");
  const secondId = await second.getAttribute("data-image");
  await second.getByRole("button", { name: "Move earlier" }).click();
  await expect(page.getByRole("status")).toContainText("Gallery reordered.");
  await expect(page.locator("[data-image]").first()).toHaveAttribute("data-image", secondId!);

  // The description of one picture changes on its own.
  await page.locator("[data-image]").first().getByLabel("What it shows").fill("Now the first one");
  await page.locator("[data-image]").first().getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Picture description saved.");
  await expect(page.locator("[data-image]").first().locator("img")).toHaveAttribute("alt", "Now the first one");

  // Removing takes the file with it.
  await page.locator("[data-image]").first().getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("status")).toContainText("Picture removed.");
  await expect(page.locator("[data-image]")).toHaveCount(1);
  await expect(page.locator("[data-image]").first().locator("img")).toHaveAttribute("src", src!);

  // An archived product keeps the gallery it had, with no way to change it.
  await page.goto(productUrl);
  await page.getByRole("button", { name: "Archive" }).click();
  await expect(page.getByRole("status")).toContainText("Product archived.");
  await productTab(page, "Pictures");
  await expect(page.locator("[data-image]")).toHaveCount(1);
  await expect(page.getByLabel("Add a picture")).toHaveCount(0);
  await expect(page.locator("[data-image]").first().getByRole("button", { name: "Remove" })).toHaveCount(0);
});

test("an owner makes a product vary by colour and size, each combination priced", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Variant Shop");
  await createTenant(page, name);

  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill("Cat Harness");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const productUrl = page.url().split("?")[0];
  await productTab(page, "Options & variants");
  await expect(page.getByText("This product does not vary.")).toBeVisible();
  await expect(page.getByLabel("Values, one a line")).toHaveAttribute("placeholder", "Beige\nBlue\nGrey");

  // Two options, then the combinations they make.
  await page.getByLabel("Add an option").fill("Colour");
  await page.getByLabel("Values, one a line").fill("Beige\nBlue");
  await page.getByRole("button", { name: "Add option" }).click();
  await expect(page.getByRole("status")).toContainText("Option added.");
  await page.getByLabel("Add an option").fill("Size");
  await page.getByLabel("Values, one a line").fill("S\nM");
  await page.getByRole("button", { name: "Add option" }).click();
  await expect(page.locator("[data-option]")).toHaveCount(2);

  // Publishing still needs something to charge for.
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page).toHaveURL(/error=product_not_priced/);

  const addVariant = async (colour: string, size: string, amount: string, sku?: string) => {
    await page.goto(`${productUrl}?tab=variants`);
    await page.getByLabel("Colour", { exact: true }).selectOption(colour);
    await page.getByLabel("Size", { exact: true }).selectOption(size);
    await page.getByLabel("Variant price").fill(amount);
    if (sku) await page.getByLabel("Variant code (optional)").fill(sku);
    await page.getByRole("button", { name: "Add variant" }).click();
  };
  await addVariant("Beige", "S", "22.90", "harness-beige-s");
  await expect(page.getByRole("status")).toContainText("Variant added.");
  await addVariant("Beige", "M", "24.90");
  await addVariant("Blue", "S", "22.90");
  await expect(page.locator("[data-variants] tbody tr")).toHaveCount(3);
  await expect(page.locator("[data-variant]").first()).toContainText("Colour: Beige · Size: S");

  // The same combination twice, and a code another variant holds, are both out.
  await addVariant("Beige", "S", "19.90");
  await expect(page).toHaveURL(/error=variant_exists/);
  await addVariant("Blue", "M", "19.90", "HARNESS-BEIGE-S");
  await expect(page).toHaveURL(/error=sku_taken/);

  // The options are settled now: no way to add one, and no way to edit one.
  await productTab(page, "Options & variants");
  await expect(page.getByLabel("Add an option")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Save option" })).toHaveCount(0);
  await expect(page.getByText("Remove every variant before changing the options.")).toBeVisible();

  // Labels still change: what the shop shows for an option and its values,
  // while the names and values an import matches on stay.
  const colourOption = page.locator("[data-option]").filter({ hasText: "Colour" });
  await page.getByLabel("Label of Colour", { exact: true }).fill("Couleur");
  await page.getByLabel("Label of Beige (Colour)").fill("Sable");
  await colourOption.getByRole("button", { name: "Save labels" }).click();
  await expect(page.getByRole("status")).toContainText("Option labels saved.");
  await expect(page.getByLabel("Label of Colour", { exact: true })).toHaveValue("Couleur");
  await expect(page.getByLabel("Label of Beige (Colour)")).toHaveValue("Sable");
  await expect(page.getByLabel("Label of Blue (Colour)")).toHaveValue("");
  await expect(page.locator("[data-variant]").first()).toContainText("Colour: Beige · Size: S");

  // Facts go in the table beside the description.
  await productTab(page, "Facts");
  await page.getByLabel("Fact name").fill("Material");
  await page.getByLabel("Fact value").fill("Nylon");
  await page.getByRole("button", { name: "Set fact" }).click();
  await expect(page.getByRole("status")).toContainText("Fact saved.");
  await expect(page.locator("[data-attribute=Material]")).toContainText("Nylon");

  // A product that varies is published without a price of its own.
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.getByRole("status")).toContainText("Product published.");
  await expect(page.locator("[data-product-status]")).toHaveAttribute("data-product-status", "published");

  // The list shows the range too, once the read model catches up.
  await expect(async () => {
    await page.goto(`/t/${slug}/products`);
    await expect(page.locator("[data-product=cat-harness] [data-product-price]")).toContainText(
      "22.90–24.90 €",
      { timeout: 1000 },
    );
  }).toPass({ timeout: 15_000 });
});

test("an owner prepares a product for a shopping feed and labels products from the list", async ({ page }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Feed Shop");
  await createTenant(page, name);

  // A mug sold as is, and a harness in one colour.
  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill("Blue Mug");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const mugUrl = page.url().split("?")[0];
  await page.goto(`/t/${slug}/products`);
  await page.getByLabel("Name").fill("Cat Harness");
  await page.getByRole("button", { name: "Create product" }).click();
  await expect(page.getByRole("status")).toContainText("Product created.");
  const harnessUrl = page.url().split("?")[0];
  await page.goto(`${harnessUrl}?tab=variants`);
  await page.getByLabel("Add an option").fill("Colour");
  await page.getByLabel("Values, one a line").fill("Beige");
  await page.getByRole("button", { name: "Add option" }).click();
  await expect(page.getByRole("status")).toContainText("Option added.");
  await page.getByLabel("Colour", { exact: true }).selectOption("Beige");
  await page.getByLabel("Variant price").fill("22.90");
  await page.getByRole("button", { name: "Add variant" }).click();
  await expect(page.getByRole("status")).toContainText("Variant added.");

  // The mug: its brand and its numbers, a GTIN checked by its last digit.
  await page.goto(mugUrl);
  await productTab(page, "Shopping feed");
  await page.getByLabel("Brand").fill("Acme");
  await page.getByLabel("GTIN", { exact: true }).fill("4006381333932");
  await page.getByRole("button", { name: "Save identifiers" }).click();
  await expect(page).toHaveURL(/tab=shopping.*error=invalid_gtin/);
  await expect(page.getByRole("alert")).toContainText("mistyped digit");
  await page.getByLabel("Brand").fill("Acme");
  await page.getByLabel("GTIN", { exact: true }).fill("4006381 333931");
  await page.getByLabel("Part number (MPN)").fill("MUG-01");
  await page.getByRole("button", { name: "Save identifiers" }).click();
  await expect(page.getByRole("status")).toContainText("Identifiers saved.");
  await expect(page.getByLabel("Brand")).toHaveValue("Acme");
  await expect(page.getByLabel("GTIN", { exact: true })).toHaveValue("4006381333931");
  // Having none is said, not left blank, and not beside a number.
  await page.getByLabel("This product has no GTIN and no part number").check();
  await page.getByRole("button", { name: "Save identifiers" }).click();
  await expect(page).toHaveURL(/error=identifier_conflict/);

  // Where a feed files it, its labels, and what a machine wrote.
  await page.getByLabel("Google product category").fill("Home & Garden > Kitchen & Dining > Tableware > Drinkware > Mugs");
  await page.getByLabel("Age group").selectOption("adult");
  await page.getByLabel("Gender").selectOption("unisex");
  await page.getByRole("button", { name: "Save classification" }).click();
  await expect(page.getByRole("status")).toContainText("Classification saved.");
  await expect(page.getByLabel("Age group")).toHaveValue("adult");
  await expect(page.getByLabel("Google product category")).toHaveValue(/Mugs$/);
  await page.getByLabel("Label 0").fill("high margin");
  await page.getByRole("button", { name: "Save labels" }).click();
  await expect(page.getByRole("status")).toContainText("Labels saved.");
  await page.getByLabel("AI wrote the description").check();
  await page.locator("[data-ai-writing]").getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Saved.");
  await expect(page.getByLabel("AI wrote the description")).toBeChecked();
  await expect(page.getByLabel("AI wrote the name")).not.toBeChecked();

  // The harness: its variant has numbers of its own, and its option is a
  // colour to a feed.
  await page.goto(`${harnessUrl}?tab=shopping`);
  await expect(page.getByLabel("GTIN", { exact: true })).toHaveCount(0);
  await page.getByLabel("GTIN of Colour: Beige").fill("036000291452");
  await page.getByLabel("Part number of Colour: Beige").fill("H-BEIGE");
  await page.locator("[data-variant-identifiers]").getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Variant identifiers saved.");
  await expect(page.getByLabel("GTIN of Colour: Beige")).toHaveValue("036000291452");
  await page.getByLabel("To a feed, Colour is").selectOption("color");
  await page.getByRole("button", { name: "Save kind" }).click();
  await expect(page.getByRole("status")).toContainText("Option kind saved.");
  await expect(page.getByLabel("To a feed, Colour is")).toHaveValue("color");

  // From the list, one label on both at once; the list comes back as it
  // stood, and the mug's other label is untouched.
  await expect(async () => {
    await page.goto(`/t/${slug}/products?status=all`);
    await expect(page.locator("[data-product]")).toHaveCount(2, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.getByRole("button", { name: "Label the ticked products" }).click();
  await expect(page).toHaveURL(/status=all.*error=no_product_ticked/);
  await page.getByLabel("Select every product listed").check();
  await expect(page.getByLabel("Select Blue Mug")).toBeChecked();
  await page.getByLabel("Campaign label").selectOption("2");
  await page.getByLabel("Its value for the ticked products").fill("winter");
  await page.getByRole("button", { name: "Label the ticked products" }).click();
  await expect(page.getByRole("status")).toContainText("Label 2 set on 2 products.");
  await expect(page).toHaveURL(/status=all/);
  for (const url of [mugUrl, harnessUrl]) {
    await page.goto(`${url}?tab=shopping`);
    await expect(page.getByLabel("Label 2")).toHaveValue("winter");
  }
  await page.goto(`${mugUrl}?tab=shopping`);
  await expect(page.getByLabel("Label 0")).toHaveValue("high margin");
});

test("a shop opens its shopping feed at a secret address, moves it and closes it", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Feed Address Shop");
  await createTenant(page, name);
  const mugUrl = await publishProduct(page, slug, "Blue Mug", "12.50");
  await page.goto(mugUrl);
  await page.getByLabel("Description", { exact: true }).fill("Holds 50 cl & more.");
  await page.getByRole("button", { name: "Save description" }).click();
  await expect(page.getByRole("status")).toContainText("Description saved.");
  await page.goto(`${mugUrl}?tab=shopping`);
  await page.getByLabel("Brand").fill("Acme");
  await page.getByRole("button", { name: "Save identifiers" }).click();
  await expect(page.getByRole("status")).toContainText("Identifiers saved.");
  await page.getByLabel("AI wrote the description").check();
  await page.locator("[data-ai-writing]").getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Saved.");
  // A plate that stays in the shop and out of the ads.
  const plateUrl = await publishProduct(page, slug, "Plate", "8.00");
  await page.goto(`${plateUrl}?tab=shopping`);
  await expect(page.getByLabel("Send this product to the shopping feed")).toBeChecked();
  await page.getByLabel("Send this product to the shopping feed").uncheck();
  await page.getByRole("button", { name: "Save feed listing" }).click();
  await expect(page.getByRole("status")).toContainText("Out of the shopping feed.");
  await expect(page.getByLabel("Send this product to the shopping feed")).not.toBeChecked();

  // Closed until opened; then it has an address nobody can guess.
  await page.goto(`/t/${slug}/products`);
  await page.getByRole("link", { name: "Shopping feed", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/feed$`));
  await expect(page.locator("[data-feed-state]")).toHaveAttribute("data-feed-state", "closed");
  await expect(page.locator("[data-feed-needs-storefront]")).toBeVisible();
  await page.getByRole("button", { name: "Open the feed" }).click();
  await expect(page.getByRole("status")).toContainText("The feed is open at a new address.");
  await expect(page.locator("[data-feed-state]")).toHaveAttribute("data-feed-state", "open");
  const first = await page.getByLabel("Feed address").inputValue();
  expect(first).toMatch(/\/feeds\/[0-9a-f]{64}\/google$/);

  // Without the shop's address a feed has no link to give: it answers
  // nothing. A request with no session reads it once the address is set.
  await expect(async () => {
    await page.reload();
    await expect(page.locator("[data-feed-products]")).toHaveAttribute("data-feed-products", "1", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  expect((await request.get(first)).status()).toBe(404);
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Where shoppers find the shop").fill("https://shop.example");
  await page.getByRole("button", { name: "Save address" }).click();
  let xml = "";
  await expect(async () => {
    const answer = await request.get(first);
    expect(answer.status()).toBe(200);
    expect(answer.headers()["content-type"]).toContain("application/xml");
    xml = await answer.text();
  }).toPass({ timeout: 15_000 });
  expect(xml).toContain('<rss version="2.0" xmlns:g="http://base.google.com/ns/1.0">');
  expect(xml.match(/<item>/g)).toHaveLength(1);
  expect(xml).toContain("<g:title>Blue Mug</g:title>");
  expect(xml).toContain("<g:link>https://shop.example/p/blue-mug</g:link>");
  expect(xml).toContain("<g:price>12.50 EUR</g:price>");
  expect(xml).toContain("<g:availability>in_stock</g:availability>");
  expect(xml).toContain("<g:brand>Acme</g:brand>");
  // What a machine wrote is sent as such, not as the plain attribute.
  expect(xml).toContain("<g:content>Holds 50 cl &amp; more.</g:content>");
  expect(xml).toContain("<g:digital_source_type>trained_algorithmic_media</g:digital_source_type>");
  expect(xml).not.toContain("<g:description>");
  expect(xml).not.toContain("Plate");
  expect(xml.trimEnd().endsWith("</rss>")).toBe(true);

  // Filed under a category, it is linked where the storefront reads it:
  // under the category's path.
  await page.goto(`/t/${slug}/categories`);
  await page.getByLabel("Name").fill("Mugs");
  await page.getByRole("button", { name: "Create category" }).click();
  await expect(page.getByRole("status")).toContainText("Category created.");
  await fileUnder(page, mugUrl, "Mugs");
  await expect(async () => {
    expect(await (await request.get(first)).text()).toContain("<g:link>https://shop.example/mugs/blue-mug</g:link>");
  }).toPass({ timeout: 15_000 });

  // The same product for a Meta and a TikTok catalogue, each in its own
  // words, and the shop's reviews for Google, under the same secret.
  await page.goto(`/t/${slug}/feed`);
  const other = (key: string) => page.locator(`[data-feed-other="${key}"]`).inputValue();
  const meta = await other("meta");
  expect(meta).toBe(first.replace(/google$/, "meta"));
  const metaXml = await (await request.get(meta)).text();
  expect(metaXml.match(/<item>/g)).toHaveLength(1);
  expect(metaXml).toContain("<g:availability>in stock</g:availability>");
  expect(metaXml).toContain("<g:description>Holds 50 cl &amp; more.</g:description>");
  expect(metaXml).not.toContain("digital_source_type");
  const tiktokXml = await (await request.get(await other("tiktok"))).text();
  expect(tiktokXml).toMatch(/<g:sku_id>[^<]+<\/g:sku_id>/);
  expect(tiktokXml).toContain("<g:title>Blue Mug</g:title>");
  const reviews = await request.get(await other("google-reviews"));
  expect(reviews.status()).toBe(200);
  const reviewsXml = await reviews.text();
  expect(reviewsXml).toContain("<version>2.4</version>");
  expect(reviewsXml).not.toContain("<review>");
  expect(reviewsXml.trimEnd().endsWith("</feed>")).toBe(true);

  // Moved, the old address stops answering; closed, the new one does too.
  await expect(page.locator("[data-feed-needs-storefront]")).toHaveCount(0);
  await page.getByRole("button", { name: "Move to a new address" }).click();
  await expect(page.getByRole("status")).toContainText("The feed is open at a new address.");
  const second = await page.getByLabel("Feed address").inputValue();
  expect(second).not.toBe(first);
  await expect(async () => {
    expect((await request.get(first)).status()).toBe(404);
    expect((await request.get(second)).status()).toBe(200);
  }).toPass({ timeout: 15_000 });
  await page.getByRole("button", { name: "Close the feed" }).click();
  await expect(page.getByRole("status")).toContainText("The feed is closed.");
  await expect(page.locator("[data-feed-state]")).toHaveAttribute("data-feed-state", "closed");
  await expect(page.getByLabel("Feed address")).toHaveCount(0);
  await expect(async () => {
    expect((await request.get(second)).status()).toBe(404);
    expect((await request.get(second.replace(/google$/, "meta"))).status()).toBe(404);
    expect((await request.get(second.replace(/google$/, "google-reviews"))).status()).toBe(404);
  }).toPass({ timeout: 15_000 });
});

test("a reduction is measured from a price the product was on sale at, and the feed sends it as a sale", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Reduction Shop");
  await createTenant(page, name);
  const mugUrl = await publishProduct(page, slug, "Blue Mug", "20.00");

  // On sale at one price only: nothing to be reduced from.
  await page.goto(mugUrl);
  await page.getByLabel("Reduced from").fill("25.00");
  await page.getByRole("button", { name: "Apply reduction" }).click();
  await expect(page).toHaveURL(/error=no_earlier_price/);
  await expect(page.getByRole("alert")).toContainText("lower its price first");

  // Lowered to 15: reduced from 20, the lowest it was on sale at, no more.
  await page.getByLabel("Price").fill("15.00");
  await page.getByRole("button", { name: "Set price" }).click();
  await expect(page.getByRole("status")).toContainText("Price set.");
  await expect(page.locator("[data-compare-at-form]")).toContainText("At most 20.00 €");
  await page.getByLabel("Reduced from").fill("25.00");
  await page.getByRole("button", { name: "Apply reduction" }).click();
  await expect(page).toHaveURL(/error=compare_at_above_earlier/);
  await page.getByLabel("Reduced from").fill("20.00");
  await page.getByRole("button", { name: "Apply reduction" }).click();
  await expect(page.getByRole("status")).toContainText("Reduction saved.");
  await expect(page.getByLabel("Reduced from")).toHaveValue("20.00");

  // The feed sends what it is down from as the price, and what a shopper
  // pays as the sale price.
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Where shoppers find the shop").fill("https://shop.example");
  await page.getByRole("button", { name: "Save address" }).click();
  await page.goto(`/t/${slug}/feed`);
  await page.getByRole("button", { name: "Open the feed" }).click();
  const address = await page.getByLabel("Feed address").inputValue();
  await expect(async () => {
    const answer = await request.get(address);
    expect(answer.status()).toBe(200);
    expect(await answer.text()).toContain("<g:price>20.00 EUR</g:price>\n<g:sale_price>15.00 EUR</g:sale_price>");
  }).toPass({ timeout: 15_000 });

  // Back at 20, there is no reduction left to show.
  await page.goto(mugUrl);
  await page.getByLabel("Price").fill("20.00");
  await page.getByRole("button", { name: "Set price" }).click();
  await expect(page.getByRole("status")).toContainText("Price set.");
  await expect(page.getByLabel("Reduced from")).toHaveValue("");
});

test("a shop says what its goods and its sales cost, reads each margin, and its feed grades them", async ({ page, request }) => {
  await registerWithEmail(page, uniqueEmail("owner"));
  const { name, slug } = uniqueTenant("Margin Shop");
  await createTenant(page, name);
  const mugUrl = await publishProduct(page, slug, "Blue Mug", "12.50");

  // What a sale costs the shop, whatever it sells.
  await page.goto(`/t/${slug}/products`);
  await page.getByRole("link", { name: "Costs", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/t/${slug}/costs$`));
  await page.getByLabel("Sending a parcel").fill("1");
  await page.getByLabel("Payment fee, in % of the amount paid").fill("1.5");
  await page.getByLabel("Payment fee, for each payment").fill("0.25");
  await page.getByRole("button", { name: "Save costs" }).click();
  await expect(page.getByRole("status")).toContainText("Costs saved.");
  await expect(page.getByLabel("Sending a parcel")).toHaveValue("1.00");
  await expect(page.getByLabel("Payment fee, in % of the amount paid")).toHaveValue("1.50");

  // A high margin starts above a medium one; label 1 of the feed says the tier.
  await page.getByLabel("Medium from, in % of the price").fill("50");
  await page.getByRole("button", { name: "Save tiers" }).click();
  await expect(page.getByRole("alert")).toContainText("A high margin starts above a medium one");
  await expect(page.getByLabel("Medium from, in % of the price")).toHaveValue("20.00");
  await page.getByLabel("Feed label that says the tier").selectOption("1");
  await page.getByRole("button", { name: "Save tiers" }).click();
  await expect(page.getByRole("status")).toContainText("Tiers saved.");
  await expect(page.getByLabel("Feed label that says the tier")).toHaveValue("1");

  // The mug: no margin until it has a cost.
  await page.goto(mugUrl);
  await productTab(page, "Cost & margin");
  const row = page.locator("[data-margin-row]");
  await expect(row).toContainText("Not told: its cost is not known.");
  await page.getByLabel("Cost of one (EUR)").fill("4");
  await page.getByRole("button", { name: "Record what it costs" }).click();
  await expect(page.getByRole("status")).toContainText("Costs saved.");
  await expect(page.getByLabel("Cost of one (EUR)")).toHaveValue("4.00");
  // 12.50, less 4.00, 1.00 to send it and 0.44 to be paid (the shop charges
  // no VAT): 7.06 left, which an ad bringing 1.78 for 1 just pays for.
  await expect(row.locator("[data-margin]")).toHaveAttribute("data-margin", "706");
  await expect(row.locator("[data-margin]")).toHaveText("7.06 €");
  await expect(row).toHaveAttribute("data-tier", "high");
  await expect(row.locator("[data-break-even]")).toHaveText("1.78");

  // What if it sold at 6.00: 0.66 left.
  await page.getByLabel("What if it sold at (EUR)").fill("6");
  await page.getByRole("button", { name: "Work it out" }).click();
  await expect(page).toHaveURL(/tab=margin/);
  await expect(page.locator("[data-what-if-shown]")).toContainText("6.00 EUR");
  await expect(row.locator("[data-margin]")).toHaveAttribute("data-margin", "66");
  await expect(row).toHaveAttribute("data-tier", "low");

  // Sent by its supplier at no cost to the shop: its own, not the shop's.
  await page.goto(`${mugUrl}?tab=margin`);
  await page.getByLabel("Sending one (EUR)").fill("0");
  await page.getByRole("button", { name: "Record these two" }).click();
  await expect(page.getByRole("status")).toContainText("Costs saved.");
  await expect(page.getByLabel("Sending one (EUR)")).toHaveValue("0.00");
  await expect(row.locator("[data-margin]")).toHaveAttribute("data-margin", "806");

  // The feed says the tier in the label given to it.
  await page.goto(`/t/${slug}/api-keys`);
  await page.getByLabel("Where shoppers find the shop").fill("https://shop.example");
  await page.getByRole("button", { name: "Save address" }).click();
  await page.goto(`/t/${slug}/feed`);
  await page.getByRole("button", { name: "Open the feed" }).click();
  await expect(page.getByRole("status")).toContainText("The feed is open at a new address.");
  const feed = await page.getByLabel("Feed address").inputValue();
  await expect(async () => {
    const answer = await request.get(feed);
    expect(answer.status()).toBe(200);
    expect(await answer.text()).toContain("<g:custom_label_1>high</g:custom_label_1>");
  }).toPass({ timeout: 15_000 });

  // From the list, a cost on every ticked product: at 13.00 the mug loses
  // money, and the feed says so.
  await expect(async () => {
    await page.goto(`/t/${slug}/products?status=all`);
    await expect(page.locator("[data-product]")).toHaveCount(1, { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await page.getByRole("button", { name: "Cost the ticked products" }).click();
  await expect(page).toHaveURL(/status=all.*error=no_product_ticked/);
  await page.getByLabel("Select every product listed").check();
  await page.getByLabel("What one of each costs the shop").fill("13");
  await page.getByRole("button", { name: "Cost the ticked products" }).click();
  await expect(page.getByRole("status")).toContainText("Cost set on 1 product.");
  await expect(page).toHaveURL(/status=all/);
  await expect(async () => {
    expect(await (await request.get(feed)).text()).toContain("<g:custom_label_1>loss</g:custom_label_1>");
  }).toPass({ timeout: 15_000 });
  await page.goto(`${mugUrl}?tab=margin`);
  await expect(row.locator("[data-break-even]")).toHaveText("None: it loses money");
});
