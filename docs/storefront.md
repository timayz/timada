# The headless storefront library

`timada-storefront` is what a merchant's own [topcoat](https://crates.io/crates/topcoat)
app depends on to sell from its timada store. It connects to the dashboard's store API
with the tenant's API key, keeps a local copy of the published catalog in the app's own
database (one Turso file), and hands pages the data; the markup is the merchant's.

## Connecting

```rust
use timada_storefront::{RouterBuilderStoreExt, Store, StoreConfig, StoreDb, StoreTls};

let db = StoreDb::open("storefront.db").await?;                    // the app's database
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
  the catalog needs; one that sells also needs `store:checkout`, which reads what
  selling needs on top (how the shop takes payment and ships, discounts, offers,
  review links) and opens the dashboard's `/store/carts/*` and `/store/orders/*`
  endpoints, where the one cart or order whose id the storefront holds is answered
  and what the shopper does is written. Neither scope appends anything over the
  store API, and neither reads a cart, an order or an invoice series: those streams
  are every shopper's. Nothing else in the tenant's store is readable through either;
  a read naming another stream fails. `store:write` appends and reads any event (it
  implies `store:checkout`); a storefront should not hold it. What such a key forges in
  the shop's store spends nothing of the platform's: the platform's mail server writes
  to a shopper about an order, and the forwarder buys an order on AliExpress, only once
  Stripe says it holds the payment the order names, opened for that shop's cart.
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
- The **local database** is opened as the dashboard opens its own: one Turso file
  (SQLite's format in a pure-Rust, in-process engine) opened for several writers at
  once, so the catalog copy, the cursor and the snapshots are written in transactions
  that run alongside the pages' reads and are run again should two writers change the
  same rows; nothing queues behind a lock. The copy is rebuilt from the store when
  lost. A `Turso` or a `timada_core::db::Db` converts too, for a test's in-memory
  database.
- `Store::connect` applies the library's tables to the local database
  (`timada_storefront::MIGRATIONS` and `migrate` exist for hosts preparing it themselves),
  connects and follows the catalog. A store that refuses the key fails it at once; a store
  that cannot be reached does not: the shop opens on the copy it has and the store is
  reached again after a growing delay (see below).

## What lives where

Events stay on the store. The app's database file holds the subscription cursor (key
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

## When timada is away

The catalog is the shop's own copy, so a dashboard (or a store API) that is down must not
take the shop down with it. Browsing goes on untouched. What goes through the dashboard
cannot: the cart, the checkout, the account, a review, a message, a newsletter sign-up.
Every library call that reaches it then fails with `StoreUnavailable` instead of the
transport's own error, so the host tells an outage apart from a bug: `is_store_unavailable`
on a page's `topcoat::Error`, `is_unavailable` on a `Store` method's `anyhow::Error`. The
refusal code `STORE_UNAVAILABLE` (`store_unavailable`) names it beside the dashboard's own
codes, for a page wording it.

What counts as the dashboard being away: the HTTP connection failing, timing out or
breaking; a proxy in front of it answering 502, 503 or 504 with a page of its own (a load
balancer's "no available server"); the store API not connected yet, or a read on it
failing. A 503 the dashboard itself wrote (`{"error":"writes_paused"}`, or
`{"error":"write_not_taken"}` for one write it did not take) is a refusal to show the
shopper, not this.

A shop started while the store API is away opens all the same: `Store::connect` logs a
warning, `GET {prefix}/status` answers 503 with `"connected": false` and
`"subscription": {"state": "connecting", ...}`, and the supervisor connects after a growing
delay (1 s to 30 s), then follows the catalog from the local cursor. A key the store
refuses still fails `connect` at once: nothing would ever work.

The example shop (`storefront/`) shows the host's side: a request layer
(`app::indisponible::UnavailablePage`) rewrites a page that failed this way to
`/indisponible`, a page of the shop's own saying the feature is momentarily unavailable
(a 503, `Cache-Control: no-store`, `Retry-After`), with the catalog's menu and search
around it; the layout shows the cart drawer with a notice instead of an empty cart and
takes the account link as signed out; the account pages tell the shopper the dashboard is
away rather than sending them to sign in again.

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

A product's and a category's page tell search engines what the merchant wrote for them
in the back office, and what the page shows a visitor where the merchant wrote nothing:

```rust
use timada_storefront::{category_seo, product_seo};

let seo = product_seo(cx, &product).await?;     // Seo { title, description }
let seo = category_seo(cx, &category).await?;
// <title>{seo.title}</title>, <meta name="description" content="{seo.description}">
```

`title` is the merchant's or the name. `description` is the merchant's, or the page's own
description on one line, cut at a word within 160 characters; it is empty when there is
neither, and the page then writes no `description` at all. Both are read apart from
`Product` and `Category`, so a listing and the menu do not carry them.

## For search engines

A sitemap and `robots.txt` name whole addresses, so the library is told where shoppers
reach the shop (`StoreConfig::public_url`, the storefront's `PUBLIC_URL`); without it
`absolute_url` returns the path alone. Which address a product or a category has is the
host's, so the library reads the slugs off the local copy and writes the files, and the
host serves them at routes of its own:

```rust
use timada_storefront::{Sitemap, SitemapIndex, absolute_url, robots_txt,
    sitemap_categories, sitemap_pages, sitemap_parts, sitemap_products};

// /sitemap.xml: an index of the files below.
let mut index = SitemapIndex::new().sitemap(&absolute_url(cx, "/sitemap-pages.xml"));
for part in 1..=sitemap_parts(cx).await? { /* index.sitemap(…?part=N) */ }
// One file of products: at most SITEMAP_PRODUCTS (10,000), oldest first.
let mut file = Sitemap::new();
for product in sitemap_products(cx, part).await? {
    file = file.url(&absolute_url(cx, &format!("/p/{}", product.slug)), Some(product.updated_at));
}
let xml = file.xml();                                  // serve as SITEMAP_CONTENT_TYPE
let txt = robots_txt(&absolute_url(cx, "/sitemap.xml"), &["/cart", "/checkout"]);
```

`sitemap_categories` gives the live categories, `sitemap_pages` the published pages of
the shop and `sitemap_articles` the blog's published articles (the 10,000 most recent),
each with its slug and when it last changed, which becomes `<lastmod>`.
Products are listed in the order of their ids, so a product stays in its file while
others are added after it. Everything is read off the local copy and answers while the
dashboard is away.

The storefront serves `/robots.txt`, `/sitemap.xml`, `/sitemap-pages.xml` (the home,
the catalogue, the support page, the categories, its own pages once published, the
blog and its articles once it has one) and
`/sitemap-products.xml?part=N`. It keeps crawlers out of what a shopper does rather than
reads (`PRIVATE_PATHS` in `storefront/src/app/sitemap.rs`: the cart, the checkout, the
account, orders, the search…) and out of filtered listings, and marks those same pages
`<meta name="robots" content="noindex">`, as it does a listing sorted another way. A
page further down a listing (`page=`) stays open: it has products of its own.

### The head of a page

What a page's `<head>` says beyond its title is the host's to write; the library gives
the content:

```rust
use timada_storefront::{ProductPageLd, SocialCard, absolute_url, breadcrumb_ld, json_ld,
    offer_terms, organization_ld, product_ld, website_ld};

let url = absolute_url(cx, &format!("/p/{}", product.slug));     // the canonical address
let card = SocialCard { title: &seo.title, description: &seo.description, url: &url,
    site_name: "Acme", image: product.image().map(|i| i.url.as_str()),
    price: Some((1990, "EUR")) }.tags();                          // Open Graph and Twitter
let data = product_ld(&product,
    &ProductPageLd { url: &url, description: &seo.description, reviews: &reviews.items,
        terms: &offer_terms(cx).await? },
    |variant| format!("{url}?v={}", variant.id));
// <script type="application/ld+json">{json_ld(&data)}</script>
```

`SocialCard::tags` gives each tag with whether its key goes in `property` (Open Graph)
or `name` (Twitter); a price makes the card a product's, a picture makes it large.
`product_ld` writes a schema.org `Product` with its offer, or a `ProductGroup` with a
`Product` per variant on sale, each with its own offer at the address the closure gives
it. It carries `aggregateRating` only when the product has published reviews, and as
`review` the ones handed to it, which must be the ones the page shows.

Every offer also says what the shop says of delivery and returns (`offer_terms`, the
shop's shipping methods and return terms as the catalog subscription copies them):

- `shippingDetails`, one `OfferShippingDetails` per shipping method priced in the
  offer's currency: its price for that offer (nothing from the method's free
  threshold), a `DefinedRegion` per country it names, and the product's delivery time
  (its own, else the shop's default) as transit days, when there is one. A method that
  ships everywhere names no country and is left out;
- `hasMerchantReturnPolicy`, when the merchant stated return terms under Shipping: the
  window in days, by mail, free or at the buyer's cost, for the countries the shipping
  methods name. A shop whose methods name no country has nowhere to state it, and none
  is written.

The checkout does not read this copy: what a shopper pays for delivery stays the
dashboard's to say. `breadcrumb_ld`
takes `(name, address)` pairs from the home down, `organization_ld` and `website_ld`
name the shop on its home page (no rating there: a shop's rating of itself is ignored).
`json_ld` writes the JSON so that nothing in it can close the script.

The storefront's layout writes on every page a `<link rel="canonical">` to the page's
own address, the card, and the data the page hands it:

- a listing filtered, searched or sorted another way names the plain listing and is
  `noindex`; each page further down keeps its own address (`?page=N`) and canonical,
  reached by plain links;
- a product's page opens on one variant at `?v=<variant id>` (its price, its picture
  first, its values picked), which the table of variants links to and the data gives
  as that variant's offer; the canonical stays the address without it;
- the data names the brand, the GTIN and the maker's part number where the merchant
  gave them (`product.brand`, `product.gtin`, `product.mpn`, and `gtin` / `mpn` on each
  `Variant`). An option the merchant said is a colour, a size, a material or a pattern
  (`ProductOption::kind`) is listed in the group's `variesBy` and written on each
  variant under that name (`"color": "Rouge"`); the other options stay free properties.
  The labels, the Google category and the audience are for a shopping feed only and do
  not reach the storefront's copy.
- a reduction: `product.compare_at_cents` (a product sold as is) and
  `variant.compare_at_cents` are the price to strike through, there only while above the
  price; `compare_at_with(&money)` writes it. The back office holds it to the lowest
  price of the thirty days before, so a storefront shows it as it is. The data gives it
  as the offer's `priceSpecification` (`StrikethroughPrice`).

### Cookie consent

Before a tag measures anything, the visitor is asked. The library holds the answer's
shape and what Google is told of it; the host keeps the cookie, as for every other:

```rust
use timada_storefront::{CONSENT_COOKIE, CONSENT_MAX_AGE_DAYS, Consent, ConsentMode,
    consent_script};

let answer: Option<Consent> = cookie(CONSENT_COOKIE).and_then(|v| Consent::parse(&v));
// first in the <head>, before any tag:
// <script>{consent_script(ConsentMode::Basic, answer)}</script>
if ConsentMode::Basic.loads(answer) { /* write the third party's script */ }
// on the banner's answer: keep Consent::ALL.cookie_value() for CONSENT_MAX_AGE_DAYS
```

A `Consent` has two purposes, `analytics` and `ads`. `consent_script` sets Consent Mode
v2's four signals to denied (for every visitor: the page does not know where its reader
is), then to the answer: `analytics_storage` follows the first purpose, `ad_storage`,
`ad_user_data` and `ad_personalization` the second. No answer is a refusal. Every tag
asks the same `Consent`: `ConsentMode::loads` before it writes a third party's script,
and the purpose it serves before it keeps anything of its own.

A shop picks one of two modes. `ConsentMode::Basic` loads nothing and sends nothing until
a purpose is allowed. `ConsentMode::Advanced` loads the tags with every signal denied:
they send pings without cookies, which Google models conversions from, and the script
adds `url_passthrough` and, while ads are refused, `ads_data_redaction`. Google asks an
advertiser for the four signals, not for a certified consent platform, so the shop's own
banner is enough.

The storefront asks at the top of every page until it is answered (`cookies.rs`):
"Tout refuser" and "Tout accepter" side by side, and "Choisir" for `/cookies`, where each
purpose has its box and which the footer links to so an answer can be changed. Each is a
plain form: no script is needed. The answer is kept six months in the `consent` cookie,
in the clear (`analytics.ads`, `analytics`, `ads` or `none`), and `CONSENT_MODE` in
`storefront/src/app/mod.rs` picks the mode.

### What brought a shopper

An order can keep where its shopper came from, for the merchant to see which campaign
sold. The library reads an arrival off a request and the host keeps it, within what the
visitor allowed:

```rust
use timada_storefront::{OrderSource, arriving_click, click_within, remember_click,
    source_within, SOURCE_MAX_AGE_DAYS};

// on each page asked for, once the visitor answered the banner:
if let Some(click) = arriving_click(cx).and_then(|click| click_within(click, consent)) {
    let source: OrderSource = remember_click(kept, click);   // keep SOURCE_MAX_AGE_DAYS
}
// at checkout:
Checkout { source: kept.and_then(|source| source_within(source, answer)), ..checkout }
```

An arrival (`Click`) is an address carrying a click id (`gclid`, `gbraid`, `wbraid`,
`fbclid`) or a campaign parameter (`utm_source`, `utm_medium`, `utm_campaign`, `utm_term`,
`utm_content`), or a page reached from another site (its address without the query, as
`referrer`); it also notes the path landed on and when. A page reached from the shop
itself, or typed in, is none. `remember_click` keeps the first arrival and moves the last.
`click_within` and `source_within` hold it to the consent: the click ids need `ads`, the
rest `analytics`, and no answer keeps nothing; an answer taken back drops what it no
longer covers.

The order keeps both arrivals (`OrderState::source`), and its page in the back office
shows them under "Where it came from", with the channel of each as `source / medium`:
the campaign's own words, else `google / cpc` for a Google Ads click, `facebook / fbclid`
for one Meta tagged, else the site it came from. The orders are counted by the last
arrival's channel (`timada_orders::orders_by_source`); an order whose storefront said
nothing counts apart.

The storefront keeps the source ninety days in a private cookie (`shopper.rs`), reads
arrivals in a layer over every page but the checkout's (a bank's page sending the
shopper back is not what brought them), and has the banner's form carry the address it
sat on and the site the visitor came from: a visitor who lands from an ad and only then
accepts is still counted.

### The Google tag

The merchant sets the shop's Google tag up in the back office, under Google tag: the
Analytics measurement id (`G-…`), the Google Ads conversion id (`AW-…`), the labels of
the conversions (a sale, an item added to the cart, a checkout begun) and whether a
sale carries the buyer's details (Enhanced Conversions). The catalog subscription
copies it, and the library gives what a page writes:

```rust
use timada_storefront::{TAG_EVENTS_SCRIPT, google_tag, google_tag_head, purchase_events,
    tag_events_json};

let tag = google_tag(cx).await?;                                  // None: no tag set up
// in the <head>, after consent_script:
if let Some(head) = tag.as_ref().and_then(|tag| google_tag_head(tag, mode, answer)) {
    // <script async src={head.src}></script><script>{head.script}</script>
    let events = purchase_events(tag.as_ref().unwrap(), &order, answer);
    // <script type="application/json" data-tag-events>{tag_events_json(&events)}</script>
    // <script>{TAG_EVENTS_SCRIPT}</script>
}
```

`google_tag_head` is `None` while the visitor's answer holds the tag back in the shop's
consent mode: nothing of Google's is written then. `purchase_events`,
`add_to_cart_events` and `begin_checkout_events` give the events as the arguments of
one `gtag(…)` call each: the Analytics event (`send_to` the measurement id) when the shop
has one, and the Google Ads `conversion` (`send_to` `AW-…/label`) when it has a label
for it. A sale carries the order's total as its value, its currency, its lines, any
shipping and coupon, and the order's id as `transaction_id`, so Google drops a repeat.
With Enhanced Conversions on and advertising allowed, `user_data` goes first: the
e-mail address, the phone and the name hashed (hex SHA-256 of the normalized value),
the street, city, postal code and country as they are.

The events are written as data and sent by `TAG_EVENTS_SCRIPT`, which also leaves
`timadaTag(document)` for a script that fetched another page to show a part of it: the
cart drawer's hands it the page it fetched, so an item added without leaving the page
is counted too.

The storefront's layout writes the tag on every page (`tag.rs`). Three events go
through it:

- `add_to_cart`: the form's handler notes the item in a cookie, and the page the shopper
  lands on next tells it, once;
- `begin_checkout`: the checkout's page, each time it opens;
- `purchase`: the order's page on its first showing after the payment, and only then
  (the return from the payment notes the order in a cookie the page takes). A visitor
  who refused the cookies is not counted later either.

Only a fake of Google's script stands in the browser suite: whether Google Ads receives
and attributes a conversion is checked in Google's own tools (Tag Assistant, the
conversion action's diagnostics), with a real tag on a real shop.

### The Meta pixel

The merchant sets the shop's Meta pixel in the back office, under Meta pixel: the
pixel's id and, optionally, the Conversions API token of that pixel. The catalog
subscription copies the id (the token never leaves the dashboard), and the library
gives what a page writes:

```rust
use timada_storefront::{PIXEL_EVENTS_SCRIPT, meta_pixel, meta_pixel_head, page_view_event,
    pixel_event_id, pixel_purchase_event, tag_events_json};

let pixel = meta_pixel(cx).await?;                                 // None: no pixel set
if let Some(head) = pixel.as_deref().and_then(|id| meta_pixel_head(id, answer)) {
    // in the <head>: <script>{head}</script>
    let events = [page_view_event(&pixel_event_id()), pixel_purchase_event(&order)];
    // <script type="application/json" data-pixel-events>{tag_events_json(&events)}</script>
    // <script>{PIXEL_EVENTS_SCRIPT}</script>
}
```

Meta has no mode that runs without cookies, so `meta_pixel_head` is `None` until the
visitor allowed advertising, whatever the shop's consent mode: nothing of Meta's is
written for anyone else. The events are the arguments of one `fbq(…)` call each, with
the event's id last: `page_view_event`, `view_content_event` (a product's page),
`pixel_add_to_cart_event`, `initiate_checkout_event` and `pixel_purchase_event`. Their
content ids are the ones the Google tag's items carry: the variant's id, or the
product's when it is sold as is, which is the id of its item in the shop's feeds, so an
ad platform matches what a visitor saw to its catalogue. An item added to the cart is written in a script of its own,
`data-pixel-events="added"`: a script that fetched a page to show a part of it calls
`timadaPixel(page, true)`, which sends that one alone, not the page view the shopper
never had.

A sale is told twice, and counted once. The order's page sends `Purchase` with the
order's id as its event id. And when the checkout hands the order the shopper's
browser (`Checkout::browser`, from `checkout_browser(cx, answer)`: the `User-Agent`,
the pixel's `_fbp` and `_fbc` cookies and the page's address, `None` unless the shopper
allowed advertising), the dashboard sends the same sale to Meta's Conversions API under
the same id, if the shop gave a token: see `docs/architecture.md`. An order without a
browser is told to nobody from the server.

The storefront's layout writes the pixel on every page (`tag.rs`), and gives the
browser to the checkout only in a shop that has a pixel.

### An address that moved

A product renamed in the back office moves to the slug of its new name, and a page or
an article of the blog can be moved to another slug. The copy remembers the slugs each one left (twenty at most,
the oldest forgotten first), and an archived product's last one, so the host can answer
an old address with a permanent redirect instead of a 404:

```rust
use timada_storefront::{MovedProduct, moved_article, moved_page, moved_product,
    redirect_for};

match moved_product(cx, old_slug).await? {
    Some(MovedProduct::Product(slug)) => { /* 308 to the product's page */ }
    Some(MovedProduct::Category(slug)) => { /* archived: 308 to its category */ }
    None => { /* a 404 */ }
}
let page_slug = moved_page(cx, old_slug).await?;            // a published page's new slug
let article_slug = moved_article(cx, old_slug).await?;      // a published article's
let to = redirect_for(cx, "/products/blue-mug").await?;     // the merchant's own list
```

Ask only once the page itself found nothing: a slug something else took since belongs
to that thing. `moved_product` gives the published product that left the slug last; for
an archived one, the first live category it was filed under, and nothing when it has
none, when it is a draft or when it was deleted. `redirect_for` reads the list a
merchant keeps under **Redirects** in the back office, for the addresses of a shop moved
from another platform: an old path (no query, a trailing slash aside) and the path of
the shop it sends to. A redirect never leads to another one.

The storefront does this in one layer (`storefront/src/app/moved.rs`): a `GET` or `HEAD`
that ends in a 404 is looked up in the merchant's list, then as a product's address
(`/p/{slug}`), an article's (`/blog/{slug}`), then as one of its own pages, and
answered with a 308. Each old address
goes to its own page, never all of them to the home, and they are kept for as long as
the product or the page is.

## Browsing the categories

```rust
use timada_storefront::{categories, category_hub, category_path, find_category,
    product_categories, related_products, require_category, require_category_in,
    search_categories};

let menu = categories(cx).await?;                     // live categories as trees
let node = require_category_in(&menu, slug)?;         // 404 when archived or unknown, no query
let category = require_category(cx, slug).await?;     // the same without a menu at hand
let crumbs = category_path(&menu, &category);         // its ancestors, read off the menu
let hub = category_hub(cx, &category.id).await?;      // guide, questions, products put first
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

`category_hub` gives what the merchant set for the category's page beside its listing:
`guide_html`, the buying guide below the products (Markdown turned into cleaned HTML,
as a page's); `questions`, what shoppers ask and the answers, in the merchant's order;
and `pinned`, the products the page shows first. Of the products the merchant picked
(eight at most) it gives those the page lists (published, and filed under the category
or below it), in the merchant's order. The host shows them ahead of the listing and
leaves them out of it:

```rust
let mut wanted = ProductQuery::default().in_category(category.id.clone());
for product in &hub.pinned {
    wanted = wanted.exclude(product.id.clone());   // every page, so none comes twice
}
let page = products(cx, &wanted).await?;           // page.total leaves them out too
```

The storefront does so on the plain listing only: sorted another way or filtered, the
order is the shopper's. It shows the guide and the questions on a category's first
page, each question opening on its answer (`<details>`), with no `FAQPage` data:
search engines stopped showing it.

## The shop's own pages

The host decides which pages it has (legal notice, terms of sale, privacy, about us)
and where: each is a route of its own (the storefront serves `/mentions-legales`,
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

## The blog

Articles are the merchant's too, written under `/t/{slug}/articles` (`timada-pages`):
informational content a shop ranks with beyond its categories, and that links into
them. A published article is copied into `storefront_articles`, and leaves it when
unpublished or deleted; the host decides where the blog is and how it looks.

```rust
use timada_storefront::{ArticleBlock, articles, has_articles, require_article};

let listed = articles(cx, page, 12).await?;   // the most recent first
// listed.articles: [ArticleCard { slug, title, summary, cover, published_at }]
// listed.page, listed.pages

let article = require_article(cx, slug).await?;   // a draft or an unknown slug is a 404
for block in &article.blocks {
    match block {
        ArticleBlock::Html(html) => { /* cleaned: write it unescaped */ }
        ArticleBlock::Product(slug) => { /* product_by_slug, then the host's own card */ }
        ArticleBlock::Category(slug) => { /* category_by_slug, a few of its products */ }
    }
}
// article.headings: [ArticleHeading { level, id, text }], for a table of contents
let linked = has_articles(cx).await?;        // whether a link to the blog leads somewhere
```

The body is the same Markdown as a page's, with two things more. Its `##` and `###`
headings carry an id each (`s-` then the heading's slug, numbered when two read the
same), listed in `headings` for a table of contents. And a line of its own reading
`[[product:blue-mug]]` or `[[category:mugs]]` names a product or a category of the shop
by its slug: the body comes cut there, so the host draws its own card in between. Only
a paragraph of the body itself counts (one quoted, listed or shown as code is text),
and a product no longer on sale is the host's to leave out.

A card's `summary` is the merchant's, or the start of the text; `cover` is one of the
article's pictures (an `Image`, as a product's), when the merchant picked one.
`published_at` is the day the article says it came out: the first time it was
published, unless the merchant gave another (an article brought from another blog
keeps its day; a day ahead is refused, and nothing is published on a schedule).
`meta_description` is the merchant's, else the summary, else the start of the text.

For search engines and shared links, `article_ld` gives the article as schema.org data
(`BlogPosting`: headline, dates, picture, the shop as author and publisher) and
`SocialCard::published_at` makes the card an article's:

```rust
use timada_storefront::{ArticleLd, article_ld};

let ld = article_ld(&ArticleLd {
    title: &article.title,
    description: &article.meta_description,
    url: &url,                          // whole addresses
    image: cover_url.as_deref(),
    published_at: article.published_at,
    updated_at: article.updated_at,
    publisher: "Ma boutique",
    publisher_url: &home,
});
```

The storefront serves the list at `/blog` (twelve a page; empty, it is `noindex`, out
of the sitemap, and the footer does not link to it) and an article at `/blog/{slug}`:
its day, summary and cover, a table of contents once it has two headings, and the
products it names as the shop's own cards (a category shows its four newest under a
link to it).

## Announcements

The short messages at the top of the shop ("-10% maintenant avec le code BIENVENUE10")
are the merchant's too, written under `/t/{slug}/announcements`: up to ten, each shown or
hidden, within an optional window, in the order the merchant set. The shown ones are
copied into `storefront_announcements`; `announcements` keeps those whose window is open
at the time of the request, so a message starts and stops on time with no event to wait
for. A message is one line with `**bold**` and at most one `[link](/path)` to a page of
the shop or an `https://` address; its `html` is escaped, everything else is text.

```rust
let live = timada_storefront::announcements(cx).await?; // [LiveAnnouncement { id, html }]
```

The storefront shows the first without JS and the others in turn with it, holding on
hover, focus or its Pause button.

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
the first, which is what a listing shows. The store keeps each picture in two WebP
sizes, and an `Image` knows both: the page rendition (`url`, `width`, `height`; at most
1200 px on its longest side) and the listing one (`thumb_url`, `thumb_width`,
`thumb_height`; at most 400 px), with its `alt`. A review's photos (`ReviewPhoto`) come
the same way.

Those addresses are only absolute once the host passes
`StoreConfig::media_url("<picture address>")`, taking the value from the tenant's API
keys page. Without it a picture keeps the bare paths below that address
(`400/<id>.webp`, `1200/<id>.webp`), which suits a host serving the files itself. The
context helpers fill the addresses in; a host reading rows directly with
`published_products` calls `Product::fill_urls(media_url(cx))` itself.

## The two components

```rust
price(product: &product, attrs: attributes! { class="price" })
product_image(image: &photo, attrs: attributes! { class="thumb" })
product_image(image: &photo, large: true, attrs: attributes! { class="photo" })
```

They render `<span data-price class="price">12.50 €</span>` and an `<img data-image
src alt width height loading="lazy" decoding="async">`: the listing rendition by
default, which is what a card, a menu or a cart line shows; with `large: true` the
page rendition, with a `srcset` naming both so a phone takes the small one (`sizes`
may be passed in `attrs`; it defaults to the viewport up to 1200 px). The library ships no other
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
it. The storefront passes its `12,50€` formatter this way.

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
provider, which holds the money on the card, and the provider tells the dashboard.
The order exists only then, `Authorized`; it is `Paid` once the money is taken, at
once for goods the shop holds, once the supplier has them for goods it buys after the
sale (`OrderState::supplier`). A storefront never creates an order, and its key
cannot.

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
currency, captureMethod: "manual" })`: the payment holds the money, and Stripe.js
refuses a form whose capture method differs from the payment's). On submit, call
`elements.submit()`, then `start_payment`, then `confirmPayment` with the client
secret it returned. The dashboard copies the key
into the tenant's store whenever the merchant changes the Stripe settings.

- Every call returns `Result<Result<T, OrderError>>`: the outer error is the store
  failing (`?` makes it a 500), the inner one a refusal to word for the shopper;
  `OrderError::code()` gives a stable code for a URL. The same calls exist as `Store`
  methods for code outside a request.
- `add_to_cart` starts a new cart when it is given none, or one already ordered,
  dropped or unknown: the dashboard drops a cart nobody touched for a while
  (`[carts] idle_days`, 30 days by default) once nothing about it can move money
  any more. A cart starts with its first item, checked first, so a refusal leaves
  no cart behind. `set_cart_quantity` (0 removes the line) and `remove_from_cart`
  change a line.
- The dashboard lets each visitor, and each key, start only so many carts within a
  window (`[store]` `carts_per_ip`, `carts_per_key`; 30 and 5000 an hour by default,
  held across its nodes): past it, `too_many_carts`, and no cart is started. A
  start refused for its item counts too: the limit is checked before the store is
  read. A shopper keeps the cart of their first add, so this holds back visitors
  that keep no cookie. `add_to_cart(cx, …)` reports the request's client address
  (`X-Shopper-IP`), so behind a reverse proxy the host must trust it
  (`.trusted_proxies(…)` on its router), or all its shoppers share one address and
  soon start no more carts. `Store::add_to_cart` takes the address itself.
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
  `VAT FR 20%`), or `None` outside the EU. It is an estimate for display, of a sale
  under the EU distance-sales threshold whatever its currency: the invoice decides,
  from the merchant's VAT standing, the year's sales abroad (one in another currency
  counted at the day's euro rate), the postal code and a buyer's VAT number.
- The order (`order_of_cart`, `order`) arrives once the provider told the dashboard the
  shopper confirmed the payment, usually a second after the browser comes back; for
  goods the shop holds, its money is taken a moment later (`status` `Paid`). Its
  `reference` (`K7Q4-MZP2`) is what a shopper quotes.
- `cancel_order(cx, order_id)` is the shopper cancelling it, while
  `OrderState::cancellable_by_shopper()` holds: its money only held, for goods a supplier
  buys once the order is placed, and the supplier not asked yet. The hold goes back,
  nothing is charged, a discount code's use returns. Once the shop is buying the goods
  the dashboard refuses (`order_in_preparation`). Offer it only on an order the shopper
  holds, as for reviews.
- Shipping: when the merchant set up shipping methods (`/t/{slug}/shipping`),
  `CartView::shipping_options` lists them, priced for the cart (`amount_cents` is 0
  once the goods, less any discount, reach a method's free-above threshold), and
  `shipping_required` is true. The shopper picks one that ships to their country
  (`ShippingOption::ships_to`, `CartView::shipping_options_to`) and the checkout names
  it in `Checkout::shipping_method`; the total is `CartView::total_with(option)` and
  `vat_estimate_with(country, option)` adds shipping's VAT, at the rates of the goods.
  A missing method, one that does not ship there or one removed since is refused with
  `shipping_unavailable`. A shop without methods ships for free and takes none. The
  storefront's checkout offers the countries its methods name, whichever they are,
  with their French names (`checkout/countries.rs`); a method that ships everywhere,
  or a shop without methods, offers the 33 it lists first.
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
  with `price_changed`. The storefront shows them as "Exclusive offer" cards above the
  checkout's payment, whose Add buttons post to `/cart/add` with `back=checkout`.
- Who a shopper is, and which orders a browser may look at, is the host's call: the
  library hands out any order by id. The storefront keeps the cart id, the payment in
  progress, the browser's orders and its last address in private cookies.

### The province and the city's lists

AliExpress refuses an address not on its own lists, which ship with the dashboard, so
a form can offer them, in every shop: `timada_storefront::places(cx,
country, postal_code, province)` answers the country's `provinces`, the `province`
the postal code names, and the `cities` of the province asked for, else of that one,
each with its `towns`. Put the last place picked in the address's city, and let the
shopper type it for a province without cities or for the city `Other`. All are empty
when the form should ask for nothing: a country without a list, or a postal code
under France that means an overseas department, which `delivery_country` then names
(`MQ`). Where the postal code names the province, `postal_code_awaited` says it is still
too short: ask for it first, and for the province only if `province` stays empty. Put
the pick in `ShippingAddress::region`. See
[aliexpress-provinces.md](aliexpress-provinces.md).

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
  shopper holds (the storefront checks its own cookie, as for the order page). The order
  must be paid (its money taken, not only held) and not cancelled and hold the product;
  one review per product and
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
  by default, held across its nodes): past it, `too_many_requests`, and no mail goes
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
  `[store]`'s window), `writes_paused` or `write_not_taken`. The limits are exported
  (`MIN_MESSAGE_LEN`, `MAX_MESSAGE_LEN`, `MAX_MESSAGE_NAME_LEN`).

## Automated mails

Every mail an automation scenario sends a customer (see `docs/automations.md`)
ends with a link to the storefront's `/emails/opt-out/{token}`. The page asks for
one click (a mail scanner opening it stops nothing), and the button, like a mail
client's one-click unsubscribe (`List-Unsubscribe-Post`, a POST to the same
address), hands the token back with `stop_automated_mails(cx, token)`
(`POST /store/automations/opt-out`, a key allowing `store:checkout`): `true` once
the address stops getting them, `false` for a link that does not check out. The
token carries no address and needs nothing stored. The storefront has
the page (`storefront/src/app/emails/opt_out/token`).

## `GET /store/status`

```json
{ "connected": true, "subscription": { "state": "running" }, "products": 12, "last_event_at": 1760000000, "lag_seconds": 0 }
```

`connected` is a request to the store made just now (503 when it fails); `subscription`
is `running`, `restarting` with the last error, or `stopped` with the reason; `products`
counts the published rows of the local copy; `lag_seconds` is how far the copy trails the
store, as of the last time the subscription recorded its cursor (`null` until it first
ran). A new replica, or one that was away, reads high and falls as it catches up: a
balancer can keep shoppers off it until it reads low.

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
lacks is fetched from the shop; `--skip-images` leaves them all out, `--limit N` loads
only the first N products), the shop gets a delivery method (France, Belgium,
Switzerland, free from 65 €) and the promo code `BIENVENUE10` (10 %), announced in the
top bar, so a storefront can sell at once, and a key holding `store:read` and `store:checkout` is minted. The command
ends with the storefront's settings on stdout:

```
STORE_TOKEN=tk_…
STORE_MEDIA_URL=https://timada.localhost/media/<tenant id>
STORE_API_URL=https://timada.localhost
STORE_TENANT_ID=<tenant id>
```

`--pages <dir>` also writes and publishes the shop's own pages, one Markdown file per
page named after its slug, opening with a header:

```
---
title: Politique de livraison
meta_description: Pays desservis, délais et frais de livraison.
---
Indiquez ici les pays que vous desservez…
```

(`meta_title` is the other optional key.) `storefront/pages` holds placeholder text for
the storefront's six pages, which `make seed` passes.

A second run skips what is already there (by slug), so a run that stopped halfway can
be started again. The command ends by bringing the back office's catalogue and pages
lists up to date, which takes a while for a large catalogue; `--no-catch-up` skips that
and leaves it to the server, which catches up when it next starts.
`make seed` wraps the command for the dev config with `--no-catch-up`, and
`LIMIT=200 make seed` loads only the first 200 products; `seeds/README.md`
says where each snapshot comes from and how to take it. `--env-file` also writes the
settings to a file: `make seed` writes them to `config/storefront.local.env`, which git
ignores and `make storefront` reads.

## The storefront

`storefront/` is a complete shop on the library, to start your own from: it has every
page a shop needs in French, and no brand and no colours. A top bar with the merchant's
announcements, a masthead with the search, the category bar, a home with the newest products,
the category tiles, the latest reviews and the four promises, listings with sort labels
and, on a category's first page, its longer text below the products, a product page
with its gallery, option menus, reassurance block, folding description and facts and
its reviews, the shop's own pages on routes of their own, a blog (`/blog`), a cart with its drawer, a
one-page checkout (steps, details, address and Stripe's Payment and Express Checkout
elements beside an editable cart with its VAT, reassurance and recent reviews), an
order confirmation where the shopper reviews what they bought, and shopper accounts
(sign-in from the checkout's "Déjà client ?" or the header, `/account` with the
orders and saved addresses, the checkout prefilled from them).

To run it, fill a tenant (`LIMIT=200` for a quick one, see "Seeding a catalogue"), then
start it with the settings the seed wrote:

```sh
OWNER=me@example.com make seed
make storefront
open https://storefront.localhost/
```

For another tenant, set `STORE_TOKEN`, `STORE_MEDIA_URL` and `STORE_API_URL` in the
environment, which wins over the file. All three are on the tenant's API keys page; to
sell, tick `store:checkout` on the key. The local copy of the catalog belongs to one
tenant: with another tenant's key, `rm storefront.db*` first, or the shop keeps showing
the old tenant's products. `make reset` removes the copy and the settings with the store.

### Making it your own

- **Who you are:** `SHOP_NAME`, `TAGLINE` and `CONTACT_EMAIL` at the top of
  `storefront/src/app/mod.rs`. What the top bar says (a promo code, a sale) is written
  in the back office, under Announcements.
- **What your tags do before a visitor answers the cookie banner:** `CONSENT_MODE`,
  beside them (see "Cookie consent"). The banner's words are in `cookies.rs`.
- **Your look:** add it to `storefront/src/app/theme.css`, which holds the layout only
  and is inlined in every page, with the sheets of the cart drawer, the menu and the
  search beside the code they style (`DRAWER_CSS`, `MENU_CSS`, `SEARCH_CSS`). The pages
  show in the browser's own colours; the layers that cover others use the system colours
  (`Canvas`), and a state never rests on a colour alone. Stripe's elements take their
  colours from `appearance` where the pages create them (`checkout/mod.rs`,
  `cart/drawer.rs`).
- **What you say:** the home's sections in `storefront/src/app/mod.rs`, the questions in
  `support_client/mod.rs`, the promises in `reassurance`, and the refusals in `say`. Your
  legal pages are written in the back office; `storefront/pages` holds placeholders to
  seed them with.
- **Your language:** the text and the addresses (`/boutique`, `/support-client`, …) are
  French and in the code, like the `12,50€` of `money`.

### Settings

`storefront --help` and `storefront serve --help` list them; each is a flag or the
environment variable named here. Without a subcommand it serves.

It listens on 3457 (`PORT`) and keeps its copy of the catalog in `storefront.db`
(`STORE_DB`; a `-wal` and a `-log` file live next to it). `STORE_ADDR`, `HOST`,
`STORE_TLS_CA` and `STORE_TLS_SERVER_NAME` (the host of `STORE_ADDR` by default) are the
other knobs, `TRUSTED_PROXIES` (comma-separated addresses or CIDR networks) names the
reverse proxies whose `X-Forwarded-For` gives the shopper's address, and
`STORE_COOKIE_KEY` (at least 32 bytes) encrypts the shopper cookies; unset, a random key
is made at startup and the cookies do not survive a restart. `PUBLIC_URL` is the address
shoppers reach the shop at (`https://shop.example`), which the sitemap and `robots.txt`
are written under; unset, it is the address listened on, over http. `RUST_LOG` sets what is
logged; by default `info`, with the search index and the store engine kept to warnings.
`storefront migrate` prepares the local database and exits, without reaching the store.

Ctrl+C or `SIGTERM` stops it, even while it is still reaching the store. It stops taking
connections, gives the requests under way `SHUTDOWN_TIMEOUT` seconds (10 by default) to
finish, then gives the catalog subscription 5 seconds to stop, and exits with code 0.

The browser suite boots it with a key minted in the dashboard on a small catalogue
made there: `tests/e2e/storefront.spec.ts` walks the home, a category, a product, the
cart, the search and a shop page, answers the cookie banner, and stops the process; `storefront-shop.spec.ts` checks
the catalogue follows live, then fills a cart, checks out, pays, and uses stock, discount
codes, delivery methods and checkout offers, checks an order keeps what brought
its shopper only as far as they allowed, that the Google tag waits for consent and
counts the cart, the checkout and the sale once, and that the Meta pixel waits for
advertising to be allowed and a sale reaches a stand-in for the Conversions API under
the id the browser sent; `storefront-accounts.spec.ts` reviews what
it bought, is invoiced at the VAT of where the goods go, and signs a returning shopper in
on a second browser.
