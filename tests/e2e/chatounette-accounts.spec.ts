import { expect, test, type Page } from "@playwright/test";
import {
  MAILDEV,
  latestCode,
  mailCount,
  mailTo,
  registerWithEmail,
  uniqueEmail,
  unescapeSlashes,
} from "./email";
import {
  addToCart,
  confirmPayment,
  connectStripe,
  emailField,
  fillAddress,
  payButton,
  paymentSucceeded,
  placeOrder,
} from "./shop";
import { ExampleApp, PNG, mintKey, publishProduct } from "./storefront";
import { FakeStripe, fakeStripeJs } from "./stripe";
import { createTenant, setLegalIdentity, uniqueTenant } from "./tenants";
import { FakeVies } from "./vies";

// What happens around an order on the Chatounette example: the buyer's
// reviews (handed in over the store API or from the order page, moderated
// in the dashboard), VAT by where the goods go, shopper accounts (a mailed
// code, the orders and saved addresses, signing out everywhere, deletion),
// the review-request mail and the shopper's language. The app runs on a
// port of its own, so this file runs beside the other Chatounette specs.

const app = new ExampleApp(
  "chatounette",
  Number(process.env.E2E_ACCOUNTS_PORT ?? 3460),
);
const SHOP = app.url;

test.beforeEach(async ({ page }) => {
  await fakeStripeJs(page);
});

test.afterEach(() => app.stop());

/// A review as a storefront hands it in: multipart, with the key.
function reviewForm(
  orderId: string,
  productId: string,
  body: string,
  photos: Buffer[],
): FormData {
  const form = new FormData();
  form.append("order_id", orderId);
  form.append("product_id", productId);
  form.append("rating", "4");
  form.append("title", "Lovely mug");
  form.append("body", body);
  for (const [n, photo] of photos.entries()) {
    form.append("photo", new Blob([photo]), `photo-${n}.png`);
  }
  return form;
}

test("a buyer's review waits for the merchant, who publishes, trims and takes it down", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Reviewed Shop");
    await createTenant(page, name);
    const mugUrl = await publishProduct(page, slug, "Blue Mug", "12.50");
    const productId = mugUrl.split("/").pop() ?? "";
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);
    const reader = await mintKey(page, slug);
    const seller = await mintKey(page, slug, true);
    await app.start(request, seller, 1);

    // One mug, paid for.
    await page.goto(`${SHOP}/p/blue-mug`);
    await addToCart(page);
    await placeOrder(page, SHOP, true);
    await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    const orderId = page.url().split("/").pop() ?? "";

    // The storefront hands the review in with a key that may sell.
    const submit = (token: string | null, form: FormData) =>
      request.post("/store/reviews", {
        headers: token ? { authorization: `Bearer ${token}` } : {},
        multipart: form,
      });
    const text = "Holds a big tea and keeps it warm.";
    expect(
      (await submit(null, reviewForm(orderId, productId, text, []))).status(),
    ).toBe(401);
    expect(
      (
        await submit("tk_unknown", reviewForm(orderId, productId, text, []))
      ).status(),
    ).toBe(401);
    expect(
      (
        await submit(reader.token, reviewForm(orderId, productId, text, []))
      ).status(),
    ).toBe(403);
    const refusal = async (form: FormData) => {
      const response = await submit(seller.token, form);
      expect(response.status()).toBe(422);
      return ((await response.json()) as { error: string }).error;
    };
    expect(await refusal(reviewForm(orderId, productId, "Too short", []))).toBe(
      "review_invalid_body",
    );
    expect(
      await refusal(
        reviewForm(orderId, productId, text, [Buffer.from("not a picture")]),
      ),
    ).toBe("review_invalid_photo");
    expect(await refusal(reviewForm(orderId, "not-bought", text, []))).toBe(
      "review_not_purchased",
    );
    expect(await refusal(reviewForm("0".repeat(64), productId, text, []))).toBe(
      "review_order_not_found",
    );
    const created = await submit(
      seller.token,
      reviewForm(orderId, productId, text, [PNG, PNG]),
    );
    expect(created.status()).toBe(201);
    const reviewId = ((await created.json()) as { id: string }).id;
    expect(await refusal(reviewForm(orderId, productId, text, []))).toBe(
      "review_already_reviewed",
    );

    // While it waits, the storefront may hand in a change, with the same key.
    const revise = (token: string | null, form: FormData) =>
      request.post("/store/reviews/revise", {
        headers: token ? { authorization: `Bearer ${token}` } : {},
        multipart: form,
      });
    expect(
      (await revise(null, reviewForm(orderId, productId, text, []))).status(),
    ).toBe(401);
    expect(
      (
        await revise(reader.token, reviewForm(orderId, productId, text, []))
      ).status(),
    ).toBe(403);
    const unknownPhoto = reviewForm(orderId, productId, text, []);
    unknownPhoto.append("keep_photo", "not-a-photo");
    const unknownRefused = await revise(seller.token, unknownPhoto);
    expect(unknownRefused.status()).toBe(422);
    expect(((await unknownRefused.json()) as { error: string }).error).toBe(
      "review_photo_not_found",
    );

    // The merchant finds it waiting, signed with the name it was shipped to.
    const row = page.locator(`[data-review="${reviewId}"]`);
    await expect(async () => {
      await page.goto(`/t/${slug}/reviews`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await expect(row).toHaveAttribute("data-review-status", "pending");
    await expect(row).toContainText("Ada L.");
    await row.getByRole("link", { name: "Lovely mug" }).click();
    await expect(page.locator("[data-review-status]")).toHaveAttribute(
      "data-review-status",
      "pending",
    );
    await expect(page.locator("[data-review-body]")).toHaveText(text);
    const photos = page.locator("[data-review-photo]");
    await expect(photos).toHaveCount(2);
    const photoUrl = async (n: number) =>
      (await photos.nth(n).locator("img").getAttribute("src")) ?? "";
    const first = await photoUrl(0);
    const second = await photoUrl(1);
    expect((await request.get(first)).status()).toBe(200);

    // One photo off; its file goes with it.
    await photos
      .first()
      .getByRole("button", { name: "Remove this photo" })
      .click();
    await expect(page.getByRole("status")).toContainText("Photo removed.");
    await expect(photos).toHaveCount(1);
    await expect(async () =>
      expect((await request.get(first)).status()).toBe(404),
    ).toPass({ timeout: 15_000 });

    // Published, it counts in the product's rating.
    await page.goto(mugUrl);
    await expect(page.locator("[data-product-rating]")).toContainText(
      "No published review",
    );
    await page.goto(`/t/${slug}/reviews`);
    await page.getByLabel("Status").selectOption("all");
    await page.getByRole("button", { name: "Filter" }).click();
    await row.getByRole("link", { name: "Lovely mug" }).click();
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.getByRole("status")).toContainText("Review published.");
    await expect(page.locator("[data-review-status]")).toHaveAttribute(
      "data-review-status",
      "published",
    );
    // Published, it can no longer be changed.
    const late = await revise(
      seller.token,
      reviewForm(orderId, productId, "Changed my mind about it.", []),
    );
    expect(late.status()).toBe(422);
    expect(((await late.json()) as { error: string }).error).toBe(
      "review_not_pending",
    );
    await expect(async () => {
      await page.goto(mugUrl);
      await expect(page.locator("[data-product-rating]")).toContainText(
        "★ 4.0 · 1 published",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
    // The next shopper reads it beside the checkout.
    await page.goto(`${SHOP}/p/blue-mug`);
    await addToCart(page);
    await expect(async () => {
      await page.goto(`${SHOP}/checkout`);
      await expect(page.locator("[data-checkout-reviews]")).toContainText(
        "Lovely mug",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
    await page.goto(mugUrl);

    // Taken down for good, with its last photo.
    await page.getByRole("link", { name: "See its reviews" }).click();
    await row.getByRole("link", { name: "Lovely mug" }).click();
    await page.getByLabel("Why it is rejected").fill("Not about the mug");
    await page.getByRole("button", { name: "Take down" }).click();
    await expect(page.getByRole("status")).toContainText("Review rejected.");
    await expect(page.locator("[data-review-reason]")).toHaveText(
      "Not about the mug",
    );
    await expect(page.getByRole("button", { name: "Publish" })).toHaveCount(0);
    await expect(async () =>
      expect((await request.get(second)).status()).toBe(404),
    ).toPass({ timeout: 15_000 });
    await expect(async () => {
      await page.goto(mugUrl);
      await expect(page.locator("[data-product-rating]")).toContainText(
        "No published review",
        { timeout: 1000 },
      );
    }).toPass({ timeout: 15_000 });
  } finally {
    await stripe.stop();
  }
});

test("a shopper reviews what they bought from the order page, shown once the shop publishes it", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Rated Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await publishProduct(page, slug, "Red Plate", "8");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);
    await app.start(request, await mintKey(page, slug, true), 2);

    // A mug and a plate, paid for.
    for (const product of ["blue-mug", "red-plate"]) {
      await page.goto(`${SHOP}/p/${product}`);
      await addToCart(page);
    }
    await placeOrder(page, SHOP, true);
    const orderUrl = await confirmPayment(
      page,
      request,
      SHOP,
      stripe,
      webhookUrl,
    );

    // Nothing reviewed yet: a form under each product of the order.
    const reviews = page.locator("[data-review-of]");
    await expect(reviews).toHaveCount(2);
    const mug = reviews.filter({ hasText: "Blue Mug" });
    const plate = reviews.filter({ hasText: "Red Plate" });

    // Too short is refused, with what was wrong.
    await plate.getByRole("radio", { name: "2 étoiles" }).check();
    await plate.getByLabel("Votre avis").fill("Meh.");
    await plate.getByRole("button", { name: "Envoyer mon avis" }).click();
    await expect(plate.getByRole("alert")).toContainText(
      "Dites-nous en un peu plus",
    );

    // The mug: four stars, a title, a text and a photo.
    await mug.getByRole("radio", { name: "4 étoiles" }).check();
    await mug.getByLabel("Titre (facultatif)").fill("Lovely mug");
    await mug
      .getByLabel("Votre avis")
      .fill("Holds a big tea and keeps it warm.");
    await mug
      .getByLabel(/^Photos/)
      .setInputFiles({ name: "mug.png", mimeType: "image/png", buffer: PNG });
    await mug.getByRole("button", { name: "Envoyer mon avis" }).click();
    await expect(mug.getByRole("status")).toContainText(
      "Votre avis a été transmis à la boutique.",
    );
    await expect(mug.locator("[data-review-status]")).toHaveAttribute(
      "data-review-status",
      "pending",
    );
    await expect(
      mug.getByRole("button", { name: "Envoyer mon avis" }),
    ).toHaveCount(0);

    // Still waiting, the shopper changes it: other words, and another photo.
    const kept = mug.locator("[data-review-photo-keep]");
    await expect(kept).toHaveCount(1);
    const firstPhoto = (await kept.locator("img").getAttribute("src")) ?? "";
    expect((await request.get(firstPhoto)).status()).toBe(200);
    await mug.getByText("Modifier mon avis").click();
    await mug.getByRole("radio", { name: "5 étoiles" }).check();
    await mug
      .locator("[data-review-edit]")
      .getByLabel("Votre avis")
      .fill("Holds a big tea and keeps it hot.");
    await kept
      .getByRole("checkbox", { name: "Conserver cette photo" })
      .uncheck();
    await mug
      .getByLabel(/^Ajouter des photos/)
      .setInputFiles({ name: "mug-2.png", mimeType: "image/png", buffer: PNG });
    await mug
      .getByRole("button", { name: "Enregistrer mes modifications" })
      .click();
    await expect(mug.getByRole("status")).toContainText(
      "Vos modifications ont été transmises à la boutique.",
    );
    await expect(mug.locator("[data-review-status]")).toHaveAttribute(
      "data-review-status",
      "pending",
    );
    await expect(mug.locator("[data-review-status] .rating")).toHaveAttribute(
      "aria-label",
      "5 sur 5",
    );
    await expect(kept).toHaveCount(1);
    const secondPhoto = (await kept.locator("img").getAttribute("src")) ?? "";
    expect(secondPhoto).not.toBe(firstPhoto);
    await expect(async () =>
      expect((await request.get(firstPhoto)).status()).toBe(404),
    ).toPass({ timeout: 15_000 });
    expect((await request.get(secondPhoto)).status()).toBe(200);

    // Waiting for the merchant, nothing shows on the shop.
    await page.goto(`${SHOP}/p/blue-mug`);
    await expect(page.locator("[data-reviews]")).toHaveAttribute(
      "data-reviews",
      "0",
    );
    await expect(page.locator("[data-product-rating]")).toHaveCount(0);

    // The merchant publishes it.
    const row = page.locator("[data-review-status=pending]", {
      hasText: "Lovely mug",
    });
    await expect(async () => {
      await page.goto(`/t/${slug}/reviews`);
      await expect(row).toHaveCount(1, { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await row.getByRole("link", { name: "Lovely mug" }).click();
    await expect(page.locator("[data-review-photo]")).toHaveCount(1);
    await expect(page.locator("[data-review-body]")).toHaveText(
      "Holds a big tea and keeps it hot.",
    );
    await page.getByRole("button", { name: "Publish" }).click();
    await expect(page.getByRole("status")).toContainText("Review published.");
    const reviewPage = page.url();

    // The merchant answers in public, then rewrites the answer.
    await page.getByLabel("Reply", { exact: true }).fill("Thank you!");
    await page.getByRole("button", { name: "Post the reply" }).click();
    await expect(page.getByRole("status")).toContainText("Reply saved.");
    await expect(page.locator("[data-review-reply]")).toHaveText("Thank you!");
    await page
      .getByLabel("Reply", { exact: true })
      .fill("Thank you, Ada! Enjoy your tea.");
    await page.getByRole("button", { name: "Update the reply" }).click();
    await expect(page.locator("[data-review-reply]")).toHaveText(
      "Thank you, Ada! Enjoy your tea.",
    );

    // On the product page with its photo, and stars on the card.
    await expect(async () => {
      await page.goto(`${SHOP}/p/blue-mug`);
      await expect(page.locator("[data-product-rating]")).toHaveAttribute(
        "data-product-rating",
        "5.0",
        {
          timeout: 1000,
        },
      );
    }).toPass({ timeout: 15_000 });
    const review = page.locator("[data-review]");
    await expect(review).toHaveCount(1);
    await expect(review).toContainText("Lovely mug");
    await expect(review).toContainText("Holds a big tea and keeps it hot.");
    await expect(review).toContainText("Ada L. · Achat vérifié");
    await expect(async () => {
      await page.goto(`${SHOP}/p/blue-mug`);
      await expect(review.locator("[data-review-reply]")).toContainText(
        "Thank you, Ada! Enjoy your tea.",
        {
          timeout: 1000,
        },
      );
    }).toPass({ timeout: 15_000 });
    const photo =
      (await review.locator("[data-review-photo]").getAttribute("src")) ?? "";
    expect((await request.get(photo)).status()).toBe(200);
    await page.goto(`${SHOP}/boutique`);
    await expect(
      page.locator("[data-product=blue-mug] [data-rating]"),
    ).toHaveAttribute("data-rating", "5.0");
    await expect(
      page.locator("[data-product=red-plate] [data-rating]"),
    ).toHaveCount(0);

    // The shopper's order page says it is out, and it can no longer be changed.
    await page.goto(orderUrl);
    await expect(mug.locator("[data-review-status]")).toHaveAttribute(
      "data-review-status",
      "published",
    );
    await expect(mug.locator("[data-review-edit]")).toHaveCount(0);
    await expect(mug.locator("[data-review-reply]")).toContainText(
      "Thank you, Ada! Enjoy your tea.",
    );

    // The answer taken off leaves the shop too.
    await page.goto(reviewPage);
    await page.getByRole("button", { name: "Delete the reply" }).click();
    await expect(page.getByRole("status")).toContainText("Reply deleted.");
    await expect(page.locator("[data-review-reply]")).toHaveCount(0);
    await expect(async () => {
      await page.goto(`${SHOP}/p/blue-mug`);
      await expect(page.locator("[data-review]")).toHaveCount(1, {
        timeout: 1000,
      });
      await expect(page.locator("[data-review-reply]")).toHaveCount(0, {
        timeout: 1000,
      });
    }).toPass({ timeout: 15_000 });
  } finally {
    await stripe.stop();
  }
});

/// The checkout's field for an EU company's VAT number: only a shop that
/// sells to businesses has one.
const vatField = (page: Page) =>
  page.getByLabel(
    "Numéro de TVA intracommunautaire (facultatif, pour une facture hors TVA)",
  );

/// From the cart to Stripe's return page, shipping to `country`, as a
/// business when a VAT number is given.
async function checkoutTo(
  page: Page,
  country: string,
  postalCode: string,
  vatNumber = "",
) {
  await page.goto(`${SHOP}/cart`);
  await page.getByRole("link", { name: "Commander" }).click();
  await expect(page.locator("[data-import-notice]")).toBeVisible();
  await emailField(page).fill("ada@example.org");
  await page.getByLabel("Nom complet").fill("Ada Lovelace");
  await page.getByLabel("Adresse", { exact: true }).fill("1 Main Street");
  await page.getByLabel("Code postal", { exact: true }).fill(postalCode);
  await page.getByLabel("Ville", { exact: true }).fill("Somewhere");
  await page.getByLabel("Pays", { exact: true }).selectOption(country);
  if (await vatField(page).count()) await vatField(page).fill(vatNumber);
  await payButton(page).click();
  await expect(page).toHaveURL(
    /\/checkout\/complete\?.*redirect_status=succeeded/,
    { timeout: 30_000 },
  );
}

test("a sale abroad is invoiced at the VAT of where the goods go", async ({
  page,
  request,
}) => {
  const stripe = new FakeStripe();
  const vies = new FakeVies();
  await stripe.start();
  await vies.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Export Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    const company = await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);
    await app.start(request, await mintKey(page, slug, true), 1);

    // Each order: a mug, paid, then told to the dashboard.
    const addMug = async () => {
      await page.goto(`${SHOP}/p/blue-mug`);
      await addToCart(page);
    };
    const pay = async () => {
      expect(stripe.lastIntent()!.amount).toBe(1250);
      await confirmPayment(page, request, SHOP, stripe, webhookUrl);
    };
    const buy = async (country: string, postalCode: string, vatNumber = "") => {
      await addMug();
      await checkoutTo(page, country, postalCode, vatNumber);
      await pay();
    };
    const year = new Date().getUTCFullYear();
    const regimeOf = (number: number) =>
      page.locator(
        `[data-document="F${year}-${String(number).padStart(6, "0")}"] [data-document-regime]`,
      );
    const expectRegime = async (number: number, regime: string) => {
      await expect(async () => {
        await page.goto(`/t/${slug}/invoices`);
        await expect(regimeOf(number)).toHaveAttribute(
          "data-document-regime",
          regime,
          { timeout: 1000 },
        );
      }).toPass({ timeout: 20_000 });
    };

    // A shop sells to consumers only until its merchant says otherwise: no
    // VAT number is asked for.
    await addMug();
    await page.goto(`${SHOP}/cart`);
    await page.getByRole("link", { name: "Commander" }).click();
    await expect(page.locator("#checkout-vat")).toHaveCount(0);
    await page.goto(`/t/${slug}/invoicing`);
    await expect(page.locator("[data-business-buyers]")).toHaveAttribute(
      "data-business-buyers",
      "off",
    );
    await page.getByRole("button", { name: "Accept business buyers" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Business buyers accepted.",
    );
    await expect(page.locator("[data-business-buyers]")).toHaveAttribute(
      "data-business-buyers",
      "on",
    );

    // A German business VIES knows: reverse charge. A bad number is refused
    // at checkout.
    await page.goto(`${SHOP}/cart`);
    await page.getByRole("link", { name: "Commander" }).click();
    await emailField(page).fill("ada@example.org");
    await page.getByLabel("Nom complet").fill("Ada Lovelace");
    await page.getByLabel("Adresse", { exact: true }).fill("1 Main Street");
    await page.getByLabel("Code postal", { exact: true }).fill("10115");
    await page.getByLabel("Ville", { exact: true }).fill("Berlin");
    await page.getByLabel("Pays", { exact: true }).selectOption("DE");
    await vatField(page).fill("US123");
    await payButton(page).click();
    await expect(page.getByRole("alert")).toContainText(
      "numéro de TVA intracommunautaire",
    );
    await checkoutTo(page, "DE", "10115", "DE123456789");
    await pay();
    await expectRegime(1, "reverse_charge");
    expect(vies.checked).toContain("DE123456789");

    // Out of the EU: an export.
    await buy("US", "10001");
    await expectRegime(2, "export");

    // Under the threshold a German consumer pays French VAT; once the
    // merchant joins the One-Stop Shop, German VAT, on the quarter's return.
    await buy("DE", "10115");
    await expectRegime(3, "distance_origin");
    await page.goto(`${company.companyUrl}/oss`);
    await expect(page.locator("[data-oss-empty]")).toBeVisible();
    await page.goto(company.companyUrl);
    await page.getByRole("checkbox", { name: /One-Stop Shop/ }).check();
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Legal identity saved.",
    );
    await buy("DE", "10115");
    await expectRegime(4, "oss");
    await expect(async () => {
      await page.goto(`${company.companyUrl}/oss`);
      await expect(
        page.locator(`[data-oss-row="DE-1900"] [data-oss-vat]`),
      ).toHaveText("2,00 €", { timeout: 1000 });
    }).toPass({ timeout: 15_000 });
    await page.goto(company.companyUrl);
    await expect(page.locator("[data-eu-distance]")).toContainText("20,92");

    // Shipped to New York, billed to a company in Berlin: the billing
    // fields show once asked for, Stripe and the invoice get the billing
    // address, and VAT still follows the goods.
    await addMug();
    await page.goto(`${SHOP}/checkout`);
    const billing = page.locator("[data-billing]");
    await expect(billing).toBeHidden();
    await emailField(page).fill("ada@example.org");
    await page.getByLabel("Nom complet").fill("Ada Lovelace");
    await page.getByLabel("Adresse", { exact: true }).fill("350 Fifth Avenue");
    await page.getByLabel("Code postal", { exact: true }).fill("10118");
    await page.getByLabel("Ville", { exact: true }).fill("New York");
    await page.getByLabel("Pays", { exact: true }).selectOption("US");
    await page
      .getByLabel("Utiliser une autre adresse de facturation (facultatif)")
      .check();
    await expect(billing).toBeVisible();
    await page
      .getByLabel("Nom ou société à facturer")
      .fill("Analytical Engines GmbH");
    await page.getByLabel("Adresse (facturation)").fill("Unter den Linden 1");
    await page.getByLabel("Ville de facturation").fill("Berlin");
    await page.getByLabel("Code postal de facturation").fill("10117");
    await page.getByLabel("Pays de facturation").selectOption("DE");
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    expect(
      JSON.parse(
        (await page.evaluate(() => sessionStorage.getItem("confirmed")))!,
      ).billing_details,
    ).toMatchObject({
      name: "Analytical Engines GmbH",
      email: "ada@example.org",
      address: {
        line1: "Unter den Linden 1",
        city: "Berlin",
        postal_code: "10117",
        country: "DE",
      },
    });
    await pay();
    await expect(page.locator("[data-order-billing]")).toContainText(
      "Analytical Engines GmbH",
    );
    await expect(page.locator("[data-order-address]")).toContainText(
      "350 Fifth Avenue",
    );
    await expectRegime(5, "export");
    const billed = page.locator(`[data-document="F${year}-000005"]`);
    await expect(billed).toContainText("Analytical Engines GmbH");
    await billed.locator("a").nth(1).click();
    await expect(page.locator("[data-order-billing]")).toContainText(
      "Unter den Linden 1",
    );
    await expect(page.locator("[data-order-address]")).toContainText(
      "New York",
    );

    // Express Checkout: the wallet's billing address, when it is not the
    // delivery one, fills the billing fields and goes to Stripe.
    await addMug();
    await page.goto(`${SHOP}/checkout`);
    await page
      .getByLabel("Utiliser une autre adresse de facturation (facultatif)")
      .uncheck();
    await page.evaluate(() => {
      void (window as any).__stripe.on["expressCheckout:confirm"]({
        billingDetails: {
          email: "ada@example.org",
          name: "Analytical Engines GmbH",
          address: {
            line1: "Unter den Linden 1",
            postal_code: "10117",
            city: "Berlin",
            country: "DE",
          },
        },
        shippingAddress: {
          name: "Ada Lovelace",
          address: {
            line1: "350 Fifth Avenue",
            postal_code: "10118",
            city: "New York",
            country: "US",
          },
        },
        paymentFailed() {},
      });
    });
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    expect(
      JSON.parse(
        (await page.evaluate(() => sessionStorage.getItem("confirmed")))!,
      ).billing_details,
    ).toMatchObject({
      name: "Analytical Engines GmbH",
      email: "ada@example.org",
      address: {
        line1: "Unter den Linden 1",
        city: "Berlin",
        postal_code: "10117",
        country: "DE",
      },
    });
  } finally {
    await vies.stop();
    await stripe.stop();
  }
});

/// Signs in on the shop from its sign-in page (already open): the address,
/// then the code the shop mailed.
async function signInToShop(page: Page, email: string, firstCode?: string) {
  const before = await mailCount(page.request, email);
  await emailField(page).fill(email);
  await page.getByRole("button", { name: "Recevoir un code" }).click();
  await expect(page).toHaveURL(/\/account\/sign-in\/code/);
  const code = await latestCode(page.request, email, before);
  if (firstCode) {
    await page.getByLabel("Code", { exact: true }).fill(firstCode);
    await page.getByRole("button", { name: "Valider" }).click();
    await expect(page.getByRole("alert")).toContainText(
      "Ce n'est pas le code que nous avons envoyé",
    );
  }
  await page.getByLabel("Code", { exact: true }).fill(code);
  await page.getByRole("button", { name: "Valider" }).click();
}

test("a returning shopper signs in with a mailed code and finds their orders and address", async ({
  page,
  request,
  browser,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Account Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await publishProduct(page, slug, "Red Plate", "8");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);
    await app.start(request, await mintKey(page, slug, true), 2);
    // The confirmations link to the orders on the shop.
    await page.goto(`/t/${slug}/api-keys`);
    await page.getByLabel("Where shoppers find the shop").fill(SHOP);
    await page.getByRole("button", { name: "Save address" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Storefront address saved.",
    );
    await expect(page.getByLabel("Where shoppers find the shop")).toHaveValue(
      SHOP,
    );
    const shopper = uniqueEmail("shopper");

    const pay = async (shop: Page) => {
      await expect(shop).toHaveURL(
        /\/checkout\/complete\?.*redirect_status=succeeded/,
        { timeout: 30_000 },
      );
      return confirmPayment(shop, request, SHOP, stripe, webhookUrl);
    };

    // A guest order first, as anyone.
    await page.goto(`${SHOP}/p/blue-mug`);
    await addToCart(page);
    await page.goto(`${SHOP}/checkout`);
    await fillAddress(page, shopper);
    await payButton(page).click();
    const guestOrder = await pay(page);

    // The shop mailed its confirmation, with a link back to the order.
    const guestOrderId = new URL(guestOrder).pathname.split("/").at(-1)!;
    const confirmation = await mailTo(request, shopper, `Your ${name} order `);
    expect(confirmation.from[0]?.name).toBe(name);
    // The mail's HTML escapes every "/" in an attribute as "&#x2f;".
    expect(unescapeSlashes(confirmation.html)).toContain(
      `${SHOP}/orders/${guestOrderId}`,
    );
    expect(confirmation.html).toContain("Blue Mug");
    await page.goto(`/t/${slug}/orders/${guestOrderId}`);
    await expect(page.locator("[data-order-confirmation]")).toHaveAttribute(
      "data-order-confirmation",
      "mailed",
    );

    // Next time, the checkout offers to sign in, and comes back to it.
    await page.goto(`${SHOP}/p/red-plate`);
    await addToCart(page);
    await page.goto(`${SHOP}/checkout`);
    await expect(page.locator("[data-signed-in]")).toHaveCount(0);
    await page
      .locator("[data-returning]")
      .getByRole("link", { name: "Cliquez ici pour vous connecter" })
      .click();
    await expect(page).toHaveURL(/\/account\/sign-in\?/);
    await signInToShop(page, shopper);
    await expect(page).toHaveURL(`${SHOP}/checkout`);
    await expect(page.locator("[data-signed-in]")).toContainText(shopper);
    await expect(page.locator("[data-returning]")).toHaveCount(0);
    await expect(emailField(page)).toHaveValue(shopper);
    await expect(emailField(page)).toHaveAttribute("readonly", "");
    await expect(page.getByLabel("Ville", { exact: true })).toHaveValue(
      "Paris",
    );
    await expect(
      page.getByLabel("Enregistrer cette adresse dans mon compte"),
    ).toBeChecked();
    // Billed to a company this time, kept on the account too.
    await page
      .getByLabel("Utiliser une autre adresse de facturation (facultatif)")
      .check();
    await page
      .getByLabel("Nom ou société à facturer")
      .fill("Analytical Engines GmbH");
    await page.getByLabel("Adresse (facturation)").fill("Unter den Linden 1");
    await page.getByLabel("Ville de facturation").fill("Berlin");
    await page.getByLabel("Code postal de facturation").fill("10117");
    await page.getByLabel("Pays de facturation").selectOption("DE");
    await expect(
      page.getByLabel(
        "Enregistrer cette adresse de facturation dans mon compte",
      ),
    ).not.toBeChecked();
    await page
      .getByLabel("Enregistrer cette adresse de facturation dans mon compte")
      .check();
    await payButton(page).click();
    await pay(page);

    // The code came from the shop.
    const sent = await mailTo(request, shopper, "Your sign-in code");
    expect(sent.subject).toBe(`Your sign-in code for ${name}`);
    expect(sent.from[0]?.name).toBe(name);

    // Another device: a wrong code is refused, the right one shows both
    // orders (the guest one too) and the address kept at checkout.
    const other = await browser.newContext();
    try {
      const device = await other.newPage();
      await fakeStripeJs(device);
      await device.goto(`${SHOP}/account`);
      await expect(device).toHaveURL(/\/account\/sign-in\?/);
      await signInToShop(device, shopper, "000000");
      await expect(device).toHaveURL(`${SHOP}/account`);
      await expect(device.locator("[data-account-email]")).toHaveText(shopper);
      await expect(async () => {
        await device.reload();
        await expect(device.locator("[data-account-order]")).toHaveCount(2, {
          timeout: 1000,
        });
      }).toPass({ timeout: 15_000 });
      await expect(device.locator("[data-saved-address]")).toHaveCount(2);
      await expect(device.locator("[data-saved-address=Paris]")).toContainText(
        "1 rue de la Paix",
      );
      await expect(device.locator("[data-saved-address=Berlin]")).toContainText(
        "Unter den Linden 1",
      );

      // The guest order opens here: it was placed with this address.
      const guestPath = new URL(guestOrder).pathname;
      await device.locator(`a[href="${guestPath}"]`).click();
      await expect(device).toHaveURL(guestOrder);
      await expect(device.getByRole("heading", { level: 1 })).toBeVisible();

      // Its checkout starts from the saved address.
      await device.goto(`${SHOP}/p/blue-mug`);
      await addToCart(device);
      await device.goto(`${SHOP}/checkout`);
      await expect(emailField(device)).toHaveValue(shopper);
      await expect(device.getByLabel("Nom complet")).toHaveValue(
        "Ada Lovelace",
      );
      await expect(device.getByLabel("Adresse", { exact: true })).toHaveValue(
        "1 rue de la Paix",
      );
      await expect(device.getByLabel("Ville", { exact: true })).toHaveValue(
        "Paris",
      );
      await expect(
        device.locator("[data-saved-address=Paris]"),
      ).toHaveAttribute("data-current", "");

      // Billed to another saved address in one click; still shipped to Paris.
      await expect(device.locator("[data-billing]")).toBeHidden();
      await device
        .locator("[data-saved-address=Berlin]")
        .getByRole("link", { name: "Facturer ici" })
        .click();
      await expect(device).toHaveURL(/[?&]billing=/);
      await expect(
        device.locator("[data-saved-address=Berlin]"),
      ).toHaveAttribute("data-billing-current", "");
      await expect(
        device.locator("[data-saved-address=Paris]"),
      ).toHaveAttribute("data-current", "");
      await expect(
        device.getByLabel(
          "Utiliser une autre adresse de facturation (facultatif)",
        ),
      ).toBeChecked();
      await expect(device.getByLabel("Nom ou société à facturer")).toHaveValue(
        "Analytical Engines GmbH",
      );
      await expect(device.getByLabel("Ville de facturation")).toHaveValue(
        "Berlin",
      );
      await expect(device.getByLabel("Pays de facturation")).toHaveValue("DE");
      await expect(device.getByLabel("Ville", { exact: true })).toHaveValue(
        "Paris",
      );
      // Shipping elsewhere keeps the billing choice.
      await device
        .locator("[data-saved-address=Berlin]")
        .getByRole("link", { name: "Livrer ici" })
        .click();
      await expect(device.getByLabel("Ville", { exact: true })).toHaveValue(
        "Berlin",
      );
      await expect(device.getByLabel("Ville de facturation")).toHaveValue(
        "Berlin",
      );

      // Signed out, an order this browser did not place asks to sign in
      // first (the confirmation's link, opened elsewhere), then shows.
      await device.goto(`${SHOP}/account`);
      await device
        .getByRole("button", { name: "Se déconnecter", exact: true })
        .click();
      await expect(device.locator("[data-account-link]")).toHaveText(
        "Se connecter",
      );
      await device.goto(guestOrder);
      await expect(device).toHaveURL(/\/account\/sign-in\?/);
      await signInToShop(device, shopper);
      await expect(device).toHaveURL(guestOrder);
      await expect(device.getByRole("heading", { level: 1 })).toBeVisible();

      // Removing the addresses, back on the first browser.
      await page.goto(`${SHOP}/account`);
      await page
        .locator("[data-saved-address=Paris]")
        .getByRole("button", { name: "Retirer" })
        .click();
      await page
        .locator("[data-saved-address=Berlin]")
        .getByRole("button", { name: "Retirer" })
        .click();
      await expect(page.locator("[data-saved-address]")).toHaveCount(0);

      // The merchant finds the shopper among the shop's customers, with both
      // orders (the guest one too) and no address left.
      await page.goto(`/t/${slug}/customers`);
      const customer = page.locator(`[data-customer="${shopper}"]`);
      await expect(async () => {
        await page.reload();
        await expect(customer.locator("[data-customer-addresses]")).toHaveText(
          "0",
          { timeout: 1000 },
        );
      }).toPass({ timeout: 15_000 });
      await expect(customer.locator("[data-customer-orders]")).toHaveText("2");
      await customer.getByRole("link", { name: "2" }).click();
      await expect(page).toHaveURL(new RegExp(`/t/${slug}/orders\\?q=`));
      await expect(page.locator("[data-order]")).toHaveCount(2);

      // Signing out everywhere else, from the first browser: the other device
      // is signed out, this one stays.
      await page.goto(`${SHOP}/account`);
      await page
        .getByRole("button", { name: "Se déconnecter partout ailleurs" })
        .click();
      await expect(page.getByRole("status")).toContainText(
        "Vous êtes déconnecté de tous les autres navigateurs.",
      );
      await expect(page.locator("[data-account-email]")).toHaveText(shopper);
      await device.goto(`${SHOP}/account`);
      await expect(device).toHaveURL(/\/account\/sign-in\?/);

      // Deleting the account: signed out, gone from the customers, while the
      // shop keeps both orders.
      await page.goto(`${SHOP}/account`);
      await page.getByRole("link", { name: "Supprimer mon compte…" }).click();
      await expect(page).toHaveURL(`${SHOP}/account/delete`);
      await page.getByRole("button", { name: "Supprimer mon compte" }).click();
      await expect(page).toHaveURL(`${SHOP}/`);
      await expect(page.locator("[data-account-link]")).toHaveText(
        "Se connecter",
      );
      await page.goto(`/t/${slug}/customers`);
      await expect(async () => {
        await page.reload();
        await expect(page.locator(`[data-customer="${shopper}"]`)).toHaveCount(
          0,
          { timeout: 1000 },
        );
      }).toPass({ timeout: 15_000 });
      await page.goto(`/t/${slug}/orders?q=${encodeURIComponent(shopper)}`);
      await expect(page.locator("[data-order]")).toHaveCount(2);

      // Signing in again starts a new account, with the orders and no address.
      await signInToShop(device, shopper);
      await expect(device).toHaveURL(`${SHOP}/account`);
      await expect(device.locator("[data-account-order]")).toHaveCount(2);
      await expect(device.locator("[data-saved-address]")).toHaveCount(0);

      // An erasure request: the merchant deletes the new account from the
      // customers. Gone from the list, signed out of the shop, orders kept.
      await page.goto(`/t/${slug}/customers`);
      const again = page.locator(`[data-customer="${shopper}"]`);
      await expect(async () => {
        await page.reload();
        await expect(again).toHaveCount(1, { timeout: 1000 });
      }).toPass({ timeout: 15_000 });
      await again.getByRole("button", { name: "Delete" }).click();
      await expect(page.getByRole("status")).toContainText("Customer deleted");
      await expect(async () => {
        await page.reload();
        await expect(page.locator(`[data-customer="${shopper}"]`)).toHaveCount(
          0,
          { timeout: 1000 },
        );
      }).toPass({ timeout: 15_000 });
      await device.goto(`${SHOP}/account`);
      await expect(device).toHaveURL(/\/account\/sign-in\?/);
      await page.goto(`/t/${slug}/orders?q=${encodeURIComponent(shopper)}`);
      await expect(page.locator("[data-order]")).toHaveCount(2);
    } finally {
      await other.close();
    }
  } finally {
    await stripe.stop();
  }
});

test("a buyer is mailed a link to review a fulfilled order, which opens it on another device", async ({
  page,
  request,
  browser,
}) => {
  const stripe = new FakeStripe();
  await stripe.start();
  try {
    await registerWithEmail(page, uniqueEmail("merchant"));
    const { name, slug } = uniqueTenant("Asking Shop");
    await createTenant(page, name);
    await publishProduct(page, slug, "Blue Mug", "12.50");
    await setLegalIdentity(page, slug);
    const webhookUrl = await connectStripe(page, slug);
    await app.start(request, await mintKey(page, slug, true), 1);

    // No storefront address yet: the Reviews page says no mail goes out.
    await page.goto(`/t/${slug}/reviews`);
    await expect(page.locator("[data-review-requests]")).toHaveAttribute(
      "data-review-requests",
      "7",
    );
    await expect(
      page.locator("[data-review-requests-no-storefront]"),
    ).toBeVisible();
    await page.goto(`/t/${slug}/api-keys`);
    await page.getByLabel("Where shoppers find the shop").fill(SHOP);
    await page.getByRole("button", { name: "Save address" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Storefront address saved.",
    );
    // Ask at once, so the test does not wait a week.
    await page.goto(`/t/${slug}/reviews`);
    await expect(
      page.locator("[data-review-requests-no-storefront]"),
    ).toHaveCount(0);
    await page.getByLabel("Days after fulfilment").fill("0");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status")).toContainText(
      "Review requests saved.",
    );
    await expect(page.locator("[data-review-requests]")).toHaveAttribute(
      "data-review-requests",
      "0",
    );

    // A guest order.
    const shopper = uniqueEmail("buyer");
    await page.goto(`${SHOP}/p/blue-mug`);
    await addToCart(page);
    await page.goto(`${SHOP}/checkout`);
    await fillAddress(page, shopper);
    await payButton(page).click();
    await expect(page).toHaveURL(
      /\/checkout\/complete\?.*redirect_status=succeeded/,
      { timeout: 30_000 },
    );
    const orderUrl = await confirmPayment(
      page,
      request,
      SHOP,
      stripe,
      webhookUrl,
    );
    const orderId = new URL(orderUrl).pathname.split("/").at(-1)!;

    // Not asked while it is only paid; asked once it is fulfilled.
    await page.goto(`/t/${slug}/orders/${orderId}`);
    await expect(page.locator("[data-order-review-request]")).toHaveAttribute(
      "data-order-review-request",
      "none",
    );
    await page.getByRole("button", { name: "Mark as fulfilled" }).click();
    await expect(page.getByRole("status")).toContainText(
      "Order marked as fulfilled.",
    );
    let link = "";
    await expect(async () => {
      const all = (await (await request.get(`${MAILDEV}/email`)).json()) as {
        to: { address: string }[];
        subject: string;
        from: { name: string }[];
        html?: string;
      }[];
      const asked = all.find(
        (m) =>
          m.to.some((to) => to.address === shopper) &&
          m.subject.startsWith("How was your"),
      );
      expect(asked).toBeDefined();
      expect(asked!.from[0]?.name).toBe(name);
      expect(asked!.html).toContain("Blue Mug");
      link = unescapeSlashes(asked!.html).match(
        new RegExp(`${SHOP}/reviews/[A-Za-z0-9_-]{43}`),
      )![0];
    }).toPass({ timeout: 30_000 });
    await page.goto(`/t/${slug}/orders/${orderId}`);
    await expect(page.locator("[data-order-review-request]")).toHaveAttribute(
      "data-order-review-request",
      "mailed",
    );

    // Another device, never signed in: the link opens the order's review form.
    const other = await (
      await browser.newContext({ ignoreHTTPSErrors: true })
    ).newPage();
    await other.goto(link);
    await expect(other).toHaveURL(`${SHOP}/orders/${orderId}#reviews`);
    const mug = other.locator("[data-review-of]", { hasText: "Blue Mug" });
    await mug.getByRole("radio", { name: "5 étoiles" }).check();
    await mug
      .getByLabel("Votre avis")
      .fill("Arrived quickly and holds a big tea.");
    await mug.getByRole("button", { name: "Envoyer mon avis" }).click();
    await expect(mug.getByRole("status")).toContainText(
      "Votre avis a été transmis à la boutique.",
    );

    // A token that opens nothing says so.
    await other.goto(`${SHOP}/reviews/not-a-token`);
    await expect(other.locator("[data-review-link-expired]")).toBeVisible();
    await other.context().close();
  } finally {
    await stripe.stop();
  }
});

// A browser that asks for French: the shop passes the language on with the
// sign-in code and the checkout, the shop's French wordings are used, and
// the order keeps the language.
test.describe("a French-speaking shopper", () => {
  test.use({ locale: "fr-FR" });

  test("is mailed in French, and the order remembers the language", async ({
    page,
    request,
  }) => {
    const stripe = new FakeStripe();
    await stripe.start();
    try {
      await registerWithEmail(page, uniqueEmail("merchant"));
      const { name, slug } = uniqueTenant("Bilingual Shop");
      await createTenant(page, name);
      await publishProduct(page, slug, "Blue Mug", "12.50");
      await setLegalIdentity(page, slug);
      const webhookUrl = await connectStripe(page, slug);

      // The shop mails in English and French, and words two mails in French.
      await page.goto(`/t/${slug}/mail`);
      await page
        .locator("[data-mail-languages]")
        .getByRole("checkbox", { name: "French" })
        .check();
      await page
        .locator("[data-mail-languages]")
        .getByRole("button", { name: "Save languages" })
        .click();
      await expect(page.getByRole("status")).toContainText("these languages");
      await page.goto(`/t/${slug}/mail/templates/shopper_code/fr`);
      await page.getByLabel("Subject").fill("Votre code {{ shop.name }}");
      await page
        .getByLabel("Body (HTML)")
        .fill("<p>Bonjour, votre code : <b>{{ code }}</b></p>");
      await page.getByRole("button", { name: "Save" }).click();
      await expect(page.getByRole("status")).toContainText("Saved");
      await page.goto(`/t/${slug}/mail/templates/order_confirmation/fr`);
      await page
        .getByLabel("Subject")
        .fill("Votre commande {{ order.reference }}");
      await page
        .getByLabel("Body (HTML)")
        .fill("<p>Merci pour votre commande chez {{ shop.name }}.</p>");
      await page.getByRole("button", { name: "Save" }).click();
      await expect(page.getByRole("status")).toContainText("Saved");

      await app.start(request, await mintKey(page, slug, true), 1);

      // Signing in: the code comes in French.
      const shopper = uniqueEmail("shopper");
      await page.goto(`${SHOP}/account/sign-in`);
      await signInToShop(page, shopper);
      await expect(page).toHaveURL(`${SHOP}/account`);
      const code = await mailTo(request, shopper, `Votre code ${name}`);
      expect(code.html).toMatch(/Bonjour, votre code : <b>\d{6}<\/b>/);

      // Ordering: so does the confirmation, and the order keeps the language.
      await page.goto(`${SHOP}/p/blue-mug`);
      await addToCart(page);
      await page.goto(`${SHOP}/checkout`);
      await page.getByLabel("Nom complet").fill("Ada Lovelace");
      await page
        .getByLabel("Adresse", { exact: true })
        .fill("1 rue de la Paix");
      await page.getByLabel("Code postal", { exact: true }).fill("75002");
      await page.getByLabel("Ville", { exact: true }).fill("Paris");
      await page.getByLabel("Pays", { exact: true }).selectOption("FR");
      await payButton(page).click();
      await expect(page).toHaveURL(
        /\/checkout\/complete\?.*redirect_status=succeeded/,
        { timeout: 30_000 },
      );
      await confirmPayment(page, request, SHOP, stripe, webhookUrl);
      const reference =
        (await page.locator("[data-order-reference]").textContent())?.trim() ??
        "";
      const confirmation = await mailTo(
        request,
        shopper,
        `Votre commande ${reference}`,
      );
      expect(confirmation.html).toContain(
        `Merci pour votre commande chez ${name}.`,
      );

      // The merchant sees the language on the order.
      const row = page.locator(`[data-order="${reference}"]`);
      await expect(async () => {
        await page.goto(`/t/${slug}/orders`);
        await expect(row).toHaveCount(1, { timeout: 1000 });
      }).toPass({ timeout: 15_000 });
      await row.getByRole("link", { name: reference }).click();
      await expect(page.locator("[data-order-language]")).toHaveText("French");
    } finally {
      await stripe.stop();
    }
  });
});
