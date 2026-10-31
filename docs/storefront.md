# The headless storefront library

`timada-storefront` is what a merchant's own [topcoat](https://crates.io/crates/topcoat)
app depends on to sell from its timada store. It connects to the dashboard's store API
with the tenant's API key, keeps a local copy of the published catalog in the app's own
SQLite database, and hands pages the data; the markup is the merchant's.

## Connecting

```rust
use timada_storefront::{RouterBuilderStoreExt, Store, StoreConfig, StoreDb, StoreTls};

let db = StoreDb::open("storefront.db", 8).await?;                 // the app's database
let config = StoreConfig::new("store.example.com:4321", token)
    .tls(StoreTls::from_ca_pem(Path::new("store-ca.pem"), "store.example.com")?)
    .prefix("/store");                                              // the default
let store = Store::connect(config, db).await?;
let router = module_router!().discover().store(store.clone()).build();
topcoat::serve(listener, router).await?;
store.shutdown().await;
```

- The **token** is created once on the tenant's *API keys* page of the dashboard and
  shown once. A key reaches its tenant's store and nothing else, as far as its scopes
  allow: `store:read` (read the catalog's streams: products, categories, stock,
  reviews, pages; run subscriptions over them) is all a storefront that only shows
  the catalog needs; one that sells also needs `store:checkout`, which appends what a
  shopper does (carts, placing an order) and nothing else, and reads what selling
  needs on top (carts, orders, how the shop takes payment and ships, discounts,
  offers, review links, invoices). Nothing else in the tenant's store is readable
  through either; a read naming another stream fails. (A checkout key still reads
  every cart and order of the shop, not only its visitor's: narrowing those is a
  follow-up.) `store:write` appends and reads any event (it implies
  `store:checkout`); a storefront should not hold it.
- The **address** is one node's store API, or a `host:port` in front of several (a
  load balancer, a geo-routed name). A name is looked up again on every reconnect, so
  when a node goes away the storefront follows the name to one that answers; any
  node serves any tenant. Locally, `store.timada.localhost:4320` is Traefik's balancer over
  the cluster nodes' store APIs (`.docker/traefik/dynamic.yaml`).
- **TLS** is optional in the library and in the dashboard's `[store_api]` section; without
  it the token travels in clear text, so plain TCP is for trusted networks only.
- The **dashboard address** (`StoreConfig::api_url`, shown on the API keys page) is
  where a storefront sells: its cart, the request to pay and the order it becomes are
  answered one at a time by their id under `/store/carts/*` and `/store/orders/*`,
  since the key may read neither the carts nor the orders of the shop through the
  store API. It is also where a storefront hands in what comes with files (shoppers'
  reviews and their photos) and signs its shoppers in, over HTTPS with the same key.
  Without it the store shows the catalog and sells nothing, shows reviews but takes
  none (`Store::takes_reviews`), and nobody signs in (`Store::signs_shoppers_in`).
- The **local database** is opened as the dashboard opens its own: `StoreDb::open`
  gives the pages a pool of read-only connections (the second argument: how many pages
  read at once, since each connection runs on a thread of its own) and every write
  (the catalog copy, the cursor, the snapshots) one connection, so writes queue
  instead of contending for SQLite's lock while holding a connection a page waits
  for. The copy is rebuilt from the store when lost, so the writer commits with
  `synchronous = NORMAL`. A single `SqlitePool` converts too, for a test's in-memory
  database.
- `Store::connect` fails at once when the store is unreachable or refuses the key. It
  applies evento's tables and `storefront_products` to the local database (`timada_storefront::
  migrations()` exists for hosts running their own migrator), then follows the catalog.

## What lives where

Events stay on the store. The app's SQLite file holds the subscription cursor (key
`storefront/catalog`, prefixed by the store's region), the projection snapshots, the
`storefront_products` rows (the gallery among them), the category tree
(`storefront_categories` and `storefront_product_categories`) and the published reviews
(`storefront_reviews`). Picture files are not
copied: they are loaded from the store's own address. One file per replica of the app: replicas never fence each
other out of the subscription, nothing accumulates on the store, and a restarted replica
resumes from its own cursor.

If the store is away long enough for the subscription to give up, the store's supervisor
restarts it from the local cursor with a growing delay (1 s to 30 s). Pages keep serving
the rows they have.

## Reading the catalog

```rust
use timada_storefront::{ProductQuery, products, require_product, price};

let page = products(cx, &ProductQuery::default().page(2).search("mug")).await?;
for product in &page.items { /* product.name, product.slug, product.price() */ }
let product = require_product(cx, slug).await?;   // 404 when not published
```

`products` lists published products, 24 a page (`per_page`, at most 100). `search`
matches the name or the slug anywhere in a word, ignoring case and accents, through a
full-text index the subscription keeps beside the rows, so a search costs what it finds
rather than the size of the catalog. A search shorter than three characters is too short
for that index and reads the rows instead.
`product_by_slug` returns `None` for a draft, an archived product or an unknown slug;
`require_product` turns that into a 404.

A query also narrows and orders the list:

```rust
use timada_storefront::{ProductQuery, ProductSort};

ProductQuery::default()
    .in_category(category.id.clone())        // this category and everything under it
    .sort(ProductSort::PriceAsc)             // Newest (the default), PriceAsc, PriceDesc
    .exclude(product.id.clone())             // never this one
    .per_page(36)
```

`ProductSort::parse` reads the value a sort `<select>` posts (`newest`, `price_asc`,
`price_desc`), and `as_str` writes it back into a link. A product with no price sorts
last either way.

## Browsing the categories

```rust
use timada_storefront::{categories, category_body, category_path, find_category,
    product_categories, related_products, require_category, require_category_in,
    search_categories};

let menu = categories(cx).await?;                     // live categories as trees
let node = require_category_in(&menu, slug)?;         // 404 when archived or unknown, no query
let category = require_category(cx, slug).await?;     // the same without a menu at hand
let crumbs = category_path(&menu, &category);         // its ancestors, read off the menu
let body = category_body(cx, &category.id).await?;    // the text below its products, or ""
let node = find_category(&menu, &category.slug);      // its node, with children and count
let hits = search_categories(&menu, "lit", 3);     // names containing it, accents aside
let also = related_products(cx, &product, 4).await?;  // same category, never itself
let filed_under = product_categories(cx, &product).await?;
```

`categories` returns `CategoryNode`s: the category, its children, and `product_count`,
which counts the published products of the whole subtree, so a top-level entry shows
everything below it. The count is worked out by the catalog subscription when a product
is filed, published or archived, or a category is moved, and stored on the category's
row, so building the menu on every page is one plain query. The same subscription keeps,
for each category, the products its page lists with the keys it sorts them by, so a page
of `in_category` with no search is read off an index in the order asked for, costing
its own size however large the store is, and its total is the stored count. `in_categories` is the narrower filter: products filed
directly under any of the ids, which is what related products use. An archived category
is in none of this; the products that were in it stay on sale.

## The shop's own pages

The host decides which pages it has (legal notice, terms of sale, privacy, about us)
and where: each is a route of its own (Chatounette serves `/mentions-legales`,
`/politique-de-livraison`, …) and its footer links to them. Their text is the
merchant's, written under `/t/{slug}/pages` (`timada-pages`) and read by slug: a
published page is copied into `storefront_pages`, and leaves it when unpublished or
deleted. The body is the merchant's Markdown turned into HTML on each read, raw HTML
shown as text, only the page's own pictures (from `STORE_MEDIA_URL`), and the whole
cleaned by an allow-list sanitiser.

```rust
#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    // A draft, or a slug the merchant has not written, is a 404.
    let page = timada_storefront::require_page(cx, "mentions-legales").await?;
    // page.title, page.html (cleaned), page.meta_title, page.meta_description
    …
}
let found = timada_storefront::search_pages(cx, "livrais", 3).await?; // [PageLink { slug, title }]
```

`meta_title` is the page's title unless the merchant set one for search engines, and
`meta_description` is theirs or the start of the text. `search_pages` matches a word of
the title or text that starts with the query, accents aside, titles first; the host
keeps the hits it has a route for.

## What a product varies by

```rust
let Some(range) = product.price_range() else { return };
range.label();                      // "22.90 €", or "22.90–24.90 €"
range.varies();                     // whether a shopper's choice changes the price

for option in &product.options { /* option.name, option.values */ }
for variant in &product.variants { /* variant.price(), variant.sku, variant.image_id */ }
product.variant(&picked);           // the variant those (option id, value) pairs name
for fact in &product.attributes { /* fact.name, fact.value */ }
```

`product.price()` is `price_range().label()`: one amount when nothing varies, a range
when it does. `options` is empty for a product that does not vary, and then `variants`
is too. `Variant::value_of` and `Variant::matches` are what a page needs to light up
the right row as a shopper changes the menus.

`product.left(variant_id)` says how many are left of the product (`None`) or of one
variant, and is `None` itself when the shop does not count it. `product.in_stock` is
false once everything counted is gone, and `is_sold_out(variant_id)` marks one sold-out
row; a page disables its add button then.

`ProductSort::PriceAsc` orders by the cheapest a product can be and `PriceDesc` by the
dearest, so a sorted list reads the way a shopper expects.

## Pictures

A product carries `images`, in the order the merchant put them; `product.image()` is
the first, which is what a listing shows. Each one knows its `alt`, `mime`, `width`
and `height`, and its `url`.

That address is only absolute once the host passes
`StoreConfig::media_url("<picture address>")`, taking the value from the tenant's API
keys page. Without it a picture keeps its bare `file` name, which suits a host serving
the files itself. The context helpers fill the addresses in; a host reading rows
directly with `published_products` calls `Product::fill_urls(media_url(cx))` itself.

## The two components

```rust
price(product: &product, attrs: attributes! { class="price" })
product_image(image: &photo, attrs: attributes! { class="photo" })
```

They render `<span data-price class="price">12.50 €</span>` and an `<img data-image
src alt width height loading="lazy" decoding="async">`. The library ships no other
markup and no CSS classes (a host's Tailwind build does not scan a dependency anyway):
everything a page shows is the merchant's own `view!` around `Product` values.

## How amounts are written

Every amount a person reads — the `price` component, a cart's totals and what a
discount takes off, a shipping option's price, an offer's price, the VAT estimate, an
order's total in a shopper's history — is written by one `MoneyFormat`. The default
is the locale-neutral `12.50 €`, and `Free` for shipping that costs nothing. A host
whose pages write prices their own way hands that way in once, and the library
follows it everywhere:

```rust
let config = StoreConfig::new(addr, token)
    .money(MoneyFormat::new(|cents, currency| my_money(cents, currency)).free("Offerte"));
```

The views carry the format (`cart.total()`, `option.price()`, `estimate.amount()`),
so a page never formats a total itself; `money(cx)` hands it to a page that formats
cents of its own, and `Product::price_with(money(cx))` writes a product's price with
it. The Chatounette example passes its `12,50€` formatter this way.

## Cart and checkout

(Setting up payments and testing them with Stripe's test mode: `docs/payments.md`.)

With a key holding `store:checkout`, a page sells as well as shows. Carts and orders
are not copied locally: each call writes to the store and reads back from it, so a
page shows its change at once, and a cart's prices come from the products in the
store, so the total a cart page shows is what the payment charges. The local copy
only adds each line's slug and picture.

Paying goes through the dashboard, which holds the merchant's payment provider
account (Stripe): a storefront asks to pay, the dashboard opens a payment and hands
back what the provider's card form needs, the shopper's browser confirms it with the
provider, and the provider tells the dashboard. The order exists only then. A
storefront never creates an order, and its key cannot.

```rust
use timada_storefront::{add_to_cart, cart, start_payment, order_of_cart, Checkout};

// The first add makes the cart; keep its id (a cookie, typically).
let cart_id = match add_to_cart(cx, current_id.as_deref(), &product.id, variant_id, 2).await? {
    Ok(cart_id) => cart_id,
    Err(refusal) => return show(refusal.code()),   // e.g. "variant_required"
};
let view = cart(cx, &cart_id).await?;              // lines, total, unavailable lines
let method = view.shipping_options_to(&shipping.country).next();   // None: free shipping
let session = start_payment(cx, &cart_id, Checkout {
    email, shipping, note,
    shipping_method: method.map(|m| m.id.clone()),
    expected_total_cents: Some(view.total_with(method)),   // the total the shopper saw
    language: request_language(cx),                        // the browser's Accept-Language
    ..Default::default()
}).await?;
// Ok(session): confirm with Stripe using session.client_secret; confirmPayment
// returns the browser to your page, which waits for order_of_cart(cx, &cart_id).
```

The payment form can show before any payment is open, with the cart and the
address on one page: `payment_setup(cx)` gives the provider and the merchant's
publishable key (`None` while the shop takes no payments), enough for Stripe's
Payment Element in its deferred mode (`elements({ mode: "payment", amount,
currency })`). On submit, call `elements.submit()`, then `start_payment`, then
`confirmPayment` with the client secret it returned. The dashboard copies the key
into the tenant's store whenever the merchant changes the Stripe settings.

- Every call returns `Result<Result<T, OrderError>>`: the outer error is the store
  failing (`?` makes it a 500), the inner one a refusal to word for the shopper;
  `OrderError::code()` gives a stable code for a URL. The same calls exist as `Store`
  methods for code outside a request.
- `add_to_cart` starts a new cart when it is given none, or one already ordered.
  `set_cart_quantity` (0 removes the line) and `remove_from_cart` change a line.
- A product that sells variants needs one (`variant_required`); one cart is one
  currency. A line whose product went off sale, or that wants more than is left
  (`out_of_stock`), shows in `CartView::unavailable`, and checkout waits until it is
  changed.
- `start_payment` waits up to fifteen seconds for the dashboard's answer. A refusal is
  `OrderError::PaymentRefused`: `price_changed` when the total moved since the page
  showed it, `product_unavailable`, or `payments_unavailable` when the merchant has set
  up no payments, the provider failed or nobody answered. `payment_session` reads an
  open payment back by its request id for the page after the redirect.
- Each `CartLineView` carries its `tax_category`. `CartView::vat_estimate(country)`
  gives the VAT the total includes when shipped there (`VatEstimate::label()` reads
  `VAT FR 20%`), or `None` outside the EU. It is an estimate for display: the invoice
  decides, from the merchant's VAT standing, the postal code and a buyer's VAT number.
- The order (`order_of_cart`, `order`) arrives once the provider confirmed the payment
  to the dashboard, usually a second after the browser comes back. Its `reference`
  (`K7Q4-MZP2`) is what a shopper quotes.
- Shipping: when the merchant set up shipping methods (`/t/{slug}/shipping`),
  `CartView::shipping_options` lists them, priced for the cart (`amount_cents` is 0
  once the goods, less any discount, reach a method's free-above threshold), and
  `shipping_required` is true. The shopper picks one that ships to their country
  (`ShippingOption::ships_to`, `CartView::shipping_options_to`) and the checkout names
  it in `Checkout::shipping_method`; the total is `CartView::total_with(option)` and
  `vat_estimate_with(country, option)` adds shipping's VAT, at the rates of the goods.
  A missing method, one that does not ship there or one removed since is refused with
  `shipping_unavailable`. A shop without methods ships for free and takes none.
- A cart takes one discount code: `apply_discount(cx, &cart_id, code)` (an unknown
  code is refused at once, `discount_unknown`) and `remove_discount`. `CartView` then
  has `subtotal_cents`, `discount` (what the code takes off) and `total_cents`; a code
  that takes nothing off right now (deactivated, outside its dates, an amount in
  another currency) stays on the cart with the reason in `discount_refused`.
  `start_payment` refuses such a cart with that code instead of charging full price,
  and also refuses `discount_exhausted` (a capped code has no use left) and
  `discount_already_used` (a code good once per e-mail). The use is held while the
  shopper pays and spent by the order.
- Checkout offers: the merchant may offer one unit of a product (or of one variant) at
  a lower price while the cart meets the offer's conditions (`/t/{slug}/offers`: it
  holds another product, the rest of it is worth a minimum, between two days).
  `CartView::offers` lists the ones the shopper may add (`OfferView`: the product and
  variant, `heading()`, `price()` and `offer_price()`, the slug and picture); adding the
  item with `add_to_cart` is all it takes. The cart then prices one unit of that line at
  the offer (`CartLineView::offer`), the other units at full price, and the order holds
  that unit as a line of its own, so the invoice bills what it cost. A discount code
  comes off the total with the offer. An offer that ends, or whose conditions stop
  holding, prices nothing any more: a payment for the total shown before is refused
  with `price_changed`. The example shows them as "Exclusive offer" cards above the
  checkout's payment, whose Add buttons post to `/cart/add` with `back=checkout`.
- Who a shopper is, and which orders a browser may look at, is the host's call: the
  library hands out any order by id. The example keeps the cart id, the payment in
  progress, the browser's orders and its last address in private cookies.

## Reviews

A buyer reviews a product from the page of an order they placed: 1 to 5 stars, an
optional title, a text of 10 to 2000 characters and up to three photos.

```rust
use timada_storefront::{PhotoUpload, ReviewDraft, review_of, reviews_of, submit_review};

// The order page: where each product's review stands, or a form.
let mine = review_of(cx, &order_id, &line.product_id).await?;   // Option<ReviewState>
let outcome = submit_review(cx, &order_id, &product_id, ReviewDraft {
    rating: 4,
    title: Some("Lovely mug".into()),
    body: "Holds a big tea and keeps it warm.".into(),
    photos: vec![PhotoUpload { file_name, bytes }],
}).await?;                                                       // Err(refusal) to show

// The product page: published reviews, newest first, ten a page.
let reviews = reviews_of(cx, &product, page).await?;
product.rating();          // Some(4.5) once it has a published review
product.review_count;
```

- The library knows nothing of who is who: offer the form only on an order the
  shopper holds (the example checks its own cookie, as for the order page). The order
  must be paid and not cancelled and hold the product; one review per product and
  order. It is signed with the first name and initial the order was shipped to.
- A review goes to the dashboard (`POST /store/reviews` at the dashboard address)
  with its photos, which the dashboard checks and keeps like product pictures. A key
  needs `store:checkout`. Refusals come back as codes to word in the shop's language
  (`ReviewRefusal::code`: `review_invalid_body`, `review_invalid_photo`,
  `review_already_reviewed`, …).
- Nothing shows until the merchant publishes it in the dashboard; a rejected review
  leaves the shop for good, photos included. `review_of` reads the store, so the
  shopper sees their review waiting at once, and the reason if it was rejected.
- `recent_reviews(cx, n)` gives the shop's latest published reviews, for a checkout
  or home page.
- Every `Product` carries `review_count` and its average (`rating()`, or
  `rating_stars()` for whole stars); `ProductSort::TopRated` puts the best rated
  first and products with no review last. `stars(n)` writes `★★★★☆`.

## Shopper accounts

A shopper account belongs to the shop. There is no sign-up and no password: the
shopper asks for a code, the dashboard mails it from the shop's name, and the first
code they enter creates the account. The library hands back a token for the host to
keep in its shopper's browser; it never keeps one itself.

```rust
use timada_storefront::{
    delete_account, request_sign_in_code, save_address, shopper, shopper_orders, sign_in,
    sign_out, sign_out_everywhere_else,
};

request_sign_in_code(cx, &email).await?;           // Ok(()) or a ShopperRefusal
let signed_in = sign_in(cx, &email, &code).await?; // Ok(ShopperSignIn { token, expires_at })
// keep signed_in.token in a private cookie, then on each request:
let me = shopper(cx, &token).await?;               // Option<Shopper>: email, addresses
let orders = shopper_orders(cx, &token).await?;    // Ok(Vec<ShopperOrder>), newest first
save_address(cx, &token, &shipping).await?;        // Ok(address id)
sign_out(cx, &token).await?;
sign_out_everywhere_else(cx, &token).await?;       // every other browser; this one stays
delete_account(cx, &token).await?;                 // its key is destroyed, sessions go, orders stay
```

- The codes, the sessions and the addresses go through the dashboard
  (`/store/shoppers/*` at the dashboard address), with a key that allows
  `store:checkout`. `shopper` asks the dashboard too (`GET /store/shoppers/me`), one
  call per request: the store holds a shopper's e-mail and addresses only sealed
  under a key of theirs that the dashboard keeps, and destroys when the account is
  deleted. Without a dashboard address, nobody is signed in.
- A refusal is a `ShopperRefusal` to word for the shopper: `wrong_code`,
  `code_expired`, `too_many_attempts` (five wrong codes), `code_recently_sent` (one a
  minute), `too_many_requests` (see below), `invalid_email`, an address refusal
  (`invalid_country`, …), `too_many_addresses` (ten are kept), or `signed_out`
  (`is_signed_out()`) for a token that no longer signs anyone in: forget it.
- A shopper's orders are every order placed with their address, guest ones too, read
  from the dashboard's order list, so one paid a second ago may still be missing. A
  host may show an order to the shopper signed in with its e-mail address
  (`order.email == shopper.email`).
- The dashboard holds each API key, and each shopper's address at a shop, to a number
  of codes within a window (`[store]` in its configuration; 100 and 10 per 10 minutes
  by default, counted on each node): past it, `too_many_requests`, and no mail goes
  out. `request_sign_in_code(cx, …)` reports the request's client address
  (`X-Shopper-IP`), so behind a reverse proxy the host must trust it
  (`.trusted_proxies(…)` on its router), or all its shoppers share one address.
  `Store::request_sign_in_code` takes the address itself.
- `request_sign_in_code(cx, …)` also reports the request's language, the primary
  subtag of its `Accept-Language` (`language=fr`); `request_language(cx)` reads it
  for `Checkout::language` too. The dashboard keeps the language on the order and
  mails the shopper in it when the shop has that wording, else in the shop's
  default language (see [mail](mail.md)). `Store::request_sign_in_code` takes the
  language itself, as its third argument.
- A session lasts 30 days. Saving an address it already keeps moves it first.
- Deleting an account forgets its addresses and ends every session, the caller's
  too: forget the token, and anything else the browser keeps about the shopper. The
  orders placed with the address stay with the shop (and in the shopper's next
  account, if they sign in again with it).

## Contact form

A shop's contact form sends a message to the merchant's back office, where it waits
under *Messages* until answered (from the merchant's own mailbox) and marked handled.
The library hands it to the dashboard and keeps nothing.

```rust
use timada_storefront::{MessageDraft, send_message, takes_messages};

if takes_messages(cx) {                          // the host gave the dashboard's address
    let sent = send_message(cx, MessageDraft { first_name, last_name, email, body }).await?;
    // Ok(message id), or Err(MessageRefusal) with a code to word
}
```

- The message goes to the dashboard (`POST /store/contact` at the dashboard
  address) with a key that allows `store:checkout`, the visitor's address (as the
  host's trusted proxies report it) and the primary subtag of their
  `Accept-Language`, so the merchant answers in their language.
- A refusal is a `MessageRefusal` to word for the visitor (`code()`):
  `message_invalid_name` (a first and a last name, 80 characters at most each),
  `message_invalid_email`, `message_invalid_body` (10 to 4000 characters),
  `too_many_requests` (the dashboard counts messages per visitor and per key within
  `[store]`'s window) or `writes_paused`. The limits are exported
  (`MIN_MESSAGE_LEN`, `MAX_MESSAGE_LEN`, `MAX_MESSAGE_NAME_LEN`).

## Automated mails

Every mail an automation scenario sends a customer (see `docs/automations.md`)
ends with a link to the storefront's `/emails/opt-out/{token}`. The page asks for
one click (a mail scanner opening it stops nothing), and the button, like a mail
client's one-click unsubscribe (`List-Unsubscribe-Post`, a POST to the same
address), hands the token back with `stop_automated_mails(cx, token)`
(`POST /store/automations/opt-out`, a key allowing `store:checkout`): `true` once
the address stops getting them, `false` for a link that does not check out. The
token carries no address and needs nothing stored. The Chatounette example has
the page (`examples/chatounette/src/app/emails/opt_out/token`).

## `GET /store/status`

```json
{ "connected": true, "subscription": { "state": "running" }, "products": 12, "last_event_at": 1760000000 }
```

`connected` is a request to the store made just now (503 when it fails); `subscription`
is `running`, `restarting` with the last error, or `stopped` with the reason; `products`
counts the published rows of the local copy.

## Seeding a catalogue

A tenant can be filled from a snapshot in `seeds/` instead of the back-office forms,
with the server stopped (two processes on one node's files would each run their own
consensus over the same journal, so the command refuses to run while the server's port
is in use). A snapshot is a directory holding `catalog.json.gz` and the pictures it
names under `pictures/`; snapshots are taken locally and not in git (`seeds/README.md`):

```sh
make snapshot.chatounette   # once: the catalogue and its pictures into seeds/chatounette
timada-dashboard --config config/dev.local.toml seed \
    --snapshot seeds/chatounette --tenant Chatounette --owner me@example.com
```

`--owner` is the e-mail of a registered user (register at `/auth/register` first); the
tenant is created under their ownership when no tenant has the slug (`--slug` to name
it). The categories and products go in through the catalog's commands, their pictures
are copied from the snapshot into the node's media directory (a picture the snapshot
lacks is fetched from the shop; `--skip-images` leaves them all out, `--limit N` makes
a quick run), the shop gets a delivery method (France, Belgium,
Switzerland, free from 65 €) and the promo code `MONSTER10` (10 %) so a storefront can
sell at once, and a key holding `store:read` and `store:checkout` is minted. The command
ends with the storefront's settings on stdout:

```
STORE_TOKEN=tk_…
STORE_MEDIA_URL=https://timada.localhost/media/<tenant id>
STORE_API_URL=https://timada.localhost
```

`--pages <dir>` also writes and publishes the shop's own pages, one Markdown file per
page named after its slug, opening with a header:

```
---
title: Politique de livraison
meta_description: Pays desservis, délais et frais de livraison.
---
Nous livrons en France, en Belgique et en Suisse…
```

(`meta_title` is the other optional key.) `examples/chatounette/pages` holds the text of
the Chatounette's six pages, which `make seed.chatounette` passes.

A second run skips what is already there (by slug), so a run that stopped halfway can
be started again. The read models of the back office catch up when the server next
starts. `make seed.chatounette` wraps the command for the dev config; `seeds/README.md`
says where each snapshot comes from and how to take it.

## The example

`examples/chatounette` is a complete app on the library, in French and in the layout
and colours of chatounette.com: a top bar with the promo code, a masthead with the
search, the category bar, a home with the newest products, the category tiles, the
latest reviews and the four promises, listings with the shop's sort labels and, on a
category's first page, its longer text below the products, a product page with its
gallery, option menus, reassurance block, folding description and facts and its
reviews, the shop's own pages on routes of their own, a cart with its drawer, a
one-page checkout (steps, details, address and Stripe's Payment and Express Checkout
elements beside an editable cart with its VAT, reassurance and recent reviews), an
order confirmation where the shopper reviews what they bought, and shopper accounts
(sign-in from the checkout's "Déjà client ?" or the header, `/account` with the
orders and saved addresses, the checkout prefilled from them). It is meant to run
against a tenant filled by `make seed.chatounette` (see "Seeding a catalogue"), which
prints the three settings:

```sh
STORE_TOKEN=tk_… STORE_MEDIA_URL=https://timada.localhost/media/<tenant id> \
    STORE_API_URL=https://timada.localhost make chatounette
open https://chatounette.localhost/
```

All three values are on the tenant's API keys page; to sell, tick `store:checkout` on
the key. It listens on 3457 (`PORT`) and keeps its copy of the catalog in
`chatounette.db` (`STORE_DB`). `STORE_ADDR`, `STORE_DB_READ_CONNECTIONS` (the read
pool, 8 by default), `HOST`, `STORE_TLS_CA` and `STORE_TLS_SERVER_NAME` (the host of
`STORE_ADDR` by default) are the other knobs, `TRUSTED_PROXIES` (comma-separated
addresses or CIDR networks) names the reverse proxies whose `X-Forwarded-For` gives
the shopper's address, and `STORE_COOKIE_KEY` (at least 32 bytes) encrypts the
shopper cookies; unset, a random key is made at startup and the cookies do not survive
a restart.

The browser suite boots it with a key minted in the dashboard on a small catalogue
made there: `tests/e2e/chatounette.spec.ts` walks the home, a category, a product, the
cart, the search and a shop page; `chatounette-shop.spec.ts` checks the catalogue
follows live, then fills a cart, checks out, pays, and uses stock, discount codes,
delivery methods and checkout offers; `chatounette-accounts.spec.ts` reviews what it
bought, is invoiced at the VAT of where the goods go, and signs a returning shopper in
on a second browser.
