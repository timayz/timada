# The headless storefront library

`timada-storefront` is what a merchant's own [topcoat](https://crates.io/crates/topcoat)
app depends on to sell from its timada store. It connects to the dashboard's store API
with the tenant's API key, keeps a local copy of the published catalog in the app's own
SQLite database, and hands pages the data; the markup is the merchant's.

## Connecting

```rust
use timada_storefront::{RouterBuilderStoreExt, Store, StoreConfig, StoreTls};

let pool = SqlitePoolOptions::new().connect_with(options).await?;   // the app's database
let config = StoreConfig::new("store.example.com:4321".parse()?, token)
    .tls(StoreTls::from_ca_pem(Path::new("store-ca.pem"), "store.example.com")?)
    .prefix("/store");                                              // the default
let store = Store::connect(config, pool).await?;
let router = module_router!().discover().store(store.clone()).build();
topcoat::serve(listener, router).await?;
store.shutdown().await;
```

- The **token** is created once on the tenant's *API keys* page of the dashboard and
  shown once. A key has full access to its tenant's store and nothing else.
- **TLS** is optional in the library and in the dashboard's `[store_api]` section; without
  it the token travels in clear text, so plain TCP is for trusted networks only.
- `Store::connect` fails at once when the store is unreachable or refuses the key. It
  applies evento's tables and `storefront_products` to the pool (`timada_storefront::
  migrations()` exists for hosts running their own migrator), then follows the catalog.

## What lives where

Events stay on the store. The app's SQLite file holds the subscription cursor (key
`storefront/catalog`, prefixed by the store's region), the projection snapshots, the
`storefront_products` rows and the category tree (`storefront_categories` and
`storefront_product_categories`). One file per replica of the app: replicas never fence each
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

`products` lists published products, 24 a page (`per_page`, at most 100).
`product_by_slug` returns `None` for a draft, an archived product or an unknown slug;
`require_product` turns that into a 404.

A query also narrows and orders the list:

```rust
use timada_storefront::{ProductQuery, ProductSort};

ProductQuery::default()
    .in_categories(node.subtree_ids())       // this category and everything under it
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
    related_products, require_category};

let menu = categories(cx).await?;                     // live categories as trees
let category = require_category(cx, slug).await?;     // 404 when archived or unknown
let crumbs = category_path(cx, &category).await?;     // its ancestors, top-level first
let node = find_category(&menu, &category.slug);      // its node, with children and count
let also = related_products(cx, &product, 4).await?;  // same category, never itself
let filed_under = product_categories(cx, &product).await?;
```

`categories` returns `CategoryNode`s: the category, its children, and `product_count`,
which counts the published products of the whole subtree, so a top-level entry shows
everything below it. `subtree_ids` gives the ids a collection page passes to
`in_categories`. An archived category is in none of this; the products that were in it
stay on sale.

## The one component

```rust
price(product: &product, attrs: attributes! { class="price" })
```

renders `<span data-price class="price">12.50 EUR</span>`. The library ships no other
markup and no CSS classes (a host's Tailwind build does not scan a dependency anyway):
everything a page shows is the merchant's own `view!` around `Product` values.

## `GET /store/status`

```json
{ "connected": true, "subscription": { "state": "running" }, "products": 12, "last_event_at": 1760000000 }
```

`connected` is a request to the store made just now (503 when it fails); `subscription`
is `running`, `restarting` with the last error, or `stopped` with the reason; `products`
counts the published rows of the local copy.

## The example

`examples/storefront` is a complete app on the library: a catalog page, a product page,
inline styles, no asset bundle. Against a dashboard whose `[store_api]` listens on
`127.0.0.1:4321`:

```sh
STORE_TOKEN=tk_… cargo run -p storefront-example
open http://127.0.0.1:3456/
```

`STORE_ADDR`, `STORE_DB`, `HOST`, `PORT`, `STORE_TLS_CA` and `STORE_TLS_SERVER_NAME` are
the other knobs. The browser suite (`tests/e2e/storefront.spec.ts`) boots it with a key
minted in the dashboard and checks the catalog follows live.
