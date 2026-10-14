# The headless storefront library

`timada-storefront` is what a merchant's own [topcoat](https://crates.io/crates/topcoat)
app depends on to sell from its timada store. It connects to the dashboard's store API
with the tenant's API key, keeps a local copy of the published catalog in the app's own
SQLite database, and hands pages the data; the markup is the merchant's.

## Connecting

```rust
use timada_storefront::{RouterBuilderStoreExt, Store, StoreConfig, StoreTls};

let pool = SqlitePoolOptions::new().connect_with(options).await?;   // the app's database
let config = StoreConfig::new("store.example.com:4321", token)
    .tls(StoreTls::from_ca_pem(Path::new("store-ca.pem"), "store.example.com")?)
    .prefix("/store");                                              // the default
let store = Store::connect(config, pool).await?;
let router = module_router!().discover().store(store.clone()).build();
topcoat::serve(listener, router).await?;
store.shutdown().await;
```

- The **token** is created once on the tenant's *API keys* page of the dashboard and
  shown once. A key reaches its tenant's store and nothing else, as far as its scopes
  allow: `store:read` (read events, run subscriptions) is all a storefront that only
  shows the catalog needs; one that sells also needs `store:checkout`, which appends
  what a shopper does (carts, placing an order) and nothing else. `store:write`
  appends any event (it implies `store:checkout`); a storefront should not hold it. A
  key created before scopes existed holds read and write.
- The **address** is one node's store API, or a `host:port` in front of several (a
  load balancer, a geo-routed name). A name is looked up again on every reconnect, so
  when a node goes away the storefront follows the name to one that answers; any
  node serves any tenant. Locally, `store.timada.localhost:4320` is Traefik's balancer over
  the cluster nodes' store APIs (`.docker/traefik/dynamic.yaml`).
- **TLS** is optional in the library and in the dashboard's `[store_api]` section; without
  it the token travels in clear text, so plain TCP is for trusted networks only.
- `Store::connect` fails at once when the store is unreachable or refuses the key. It
  applies evento's tables and `storefront_products` to the pool (`timada_storefront::
  migrations()` exists for hosts running their own migrator), then follows the catalog.

## What lives where

Events stay on the store. The app's SQLite file holds the subscription cursor (key
`storefront/catalog`, prefixed by the store's region), the projection snapshots, the
`storefront_products` rows (the gallery among them) and the category tree
(`storefront_categories` and `storefront_product_categories`). Picture files are not
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
use timada_storefront::{categories, category_path, find_category, product_categories,
    related_products, require_category, require_category_in};

let menu = categories(cx).await?;                     // live categories as trees
let node = require_category_in(&menu, slug)?;         // 404 when archived or unknown, no query
let category = require_category(cx, slug).await?;     // the same without a menu at hand
let crumbs = category_path(&menu, &category);         // its ancestors, read off the menu
let node = find_category(&menu, &category.slug);      // its node, with children and count
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

## What a product varies by

```rust
let Some(range) = product.price_range() else { return };
range.label();                      // "22.90 EUR", or "22.90–24.90 EUR"
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

They render `<span data-price class="price">12.50 EUR</span>` and an `<img data-image
src alt width height loading="lazy" decoding="async">`. The library ships no other
markup and no CSS classes (a host's Tailwind build does not scan a dependency anyway):
everything a page shows is the merchant's own `view!` around `Product` values.

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
let session = start_payment(cx, &cart_id, Checkout {
    email, shipping, note,
    expected_total_cents: Some(view.total_cents),   // the total the shopper saw
}).await?;
// Ok(session): mount Stripe's Payment Element with session.publishable_key and
// session.client_secret; confirmPayment returns the browser to your page, which
// waits for order_of_cart(cx, &cart_id) to be Some.
```

- Every call returns `Result<Result<T, OrdersError>>`: the outer error is the store
  failing (`?` makes it a 500), the inner one a refusal to word for the shopper;
  `OrdersError::code()` gives a stable code for a URL. The same calls exist as `Store`
  methods for code outside a request.
- `add_to_cart` starts a new cart when it is given none, or one already ordered.
  `set_cart_quantity` (0 removes the line) and `remove_from_cart` change a line.
- A product that sells variants needs one (`variant_required`); one cart is one
  currency. A line whose product went off sale shows in `CartView::unavailable`, and
  checkout waits until it is removed.
- `start_payment` waits up to fifteen seconds for the dashboard's answer. A refusal is
  `OrdersError::PaymentRefused`: `price_changed` when the total moved since the page
  showed it, `product_unavailable`, or `payments_unavailable` when the merchant has set
  up no payments, the provider failed or nobody answered. `payment_session` reads an
  open payment back by its request id for the page after the redirect.
- The order (`order_of_cart`, `order`) arrives once the provider confirmed the payment
  to the dashboard, usually a second after the browser comes back. Its `reference`
  (`K7Q4-MZP2`) is what a shopper quotes.
- Who a shopper is, and which orders a browser may look at, is the host's call: the
  library hands out any order by id. The example keeps the cart id, the payment in
  progress, the browser's orders and its last address in private cookies.

## `GET /store/status`

```json
{ "connected": true, "subscription": { "state": "running" }, "products": 12, "last_event_at": 1760000000 }
```

`connected` is a request to the store made just now (503 when it fails); `subscription`
is `running`, `restarting` with the last error, or `stopped` with the reason; `products`
counts the published rows of the local copy.

## The example

`examples/storefront` is a complete app on the library: a catalog page, a category
page, a product page with an add-to-cart form, a cart, a checkout, a payment page
with Stripe's card form, an order confirmation, inline styles, no asset bundle. Against a dashboard whose
`[store_api]` listens on `127.0.0.1:4321`:

```sh
STORE_TOKEN=tk_… STORE_MEDIA_URL=https://timada.localhost/media/<tenant id> \
    cargo run -p storefront-example
open http://127.0.0.1:3456/
```

Both values are on the tenant's API keys page. `STORE_ADDR`, `STORE_DB`, `HOST`,
`PORT`, `STORE_TLS_CA` and `STORE_TLS_SERVER_NAME` (the host of `STORE_ADDR` by
default) are the other knobs, and `STORE_COOKIE_KEY` (at least 32 bytes) encrypts the
shopper cookies; unset, a random key is made at startup and the cookies do not survive
a restart. To sell, tick `store:checkout` on the key. The browser suite
(`tests/e2e/storefront.spec.ts`) boots it with a key minted in the dashboard, checks the
catalog follows live, then fills a cart and checks out.
