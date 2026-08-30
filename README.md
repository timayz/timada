# Timada

A customizable e-commerce + dropshipping framework in Rust, split by service of
concern. It covers the customer journey from arriving on the store to getting
the product delivered, with admin UIs you can mount into your own axum app.

## Stack

- **Web**: Axum + Askama + Tailwind CSS v4 (standalone CLI, no Node) + TwinSpark, mobile-first
- **Domain**: DDD / CQRS / Event Sourcing / Sagas on [evento](https://github.com/timayz/evento)
- **Persistence**: SQLite via sqlx (split read/write pools), sea-query, sqlx_migrator

## Crate map

| Crate | Responsibility |
|---|---|
| `timada-core` | Shared kernel: `Money`, ULID ids, errors, SQLite pool builders, evento executor setup, `ServiceContext` |
| `timada-web` | Shared web plumbing: base layouts, embedded assets (TwinSpark, Tailwind CSS), asset router |
| `timada-auth` | Argon2 password hashing, DB-backed sessions, admin users, admin login UI, `require_admin` middleware |
| `timada-customer` | `Customer` aggregate, registration/login/logout, the storefront account page |
| `timada-catalog` | Products: import from supplier, publish, per-currency prices, region-aware storefront + admin |
| `timada-cart` | Guest shopping cart (cookie-based), currency-locked, discount codes, TwinSpark fragments |
| `timada-region` | `Region` aggregate (currency zones, per-country VAT), storefront region picker, `RegionVat` calculator |
| `timada-promotion` | `Discount` aggregate: codes with windows and usage limits, atomic redemption counter, admin |
| `timada-order` | Checkout (discount redemption, customer linkage), `Order` aggregate, the fulfillment saga, order history, admin |
| `timada-payment` | `PaymentProvider` trait, `Payment` aggregate, built-in `FakePaymentProvider` |
| `timada-tax` | `TaxCalculator` trait, built-in `FixedRateVat` (tax-inclusive, discount-aware) |
| `timada-invoice` | `Invoice` aggregate: sequential numbering, discounts printed, credit notes on cancel and on return, printable HTML documents |
| `timada-shipping` | `Shipment` aggregate, tracking refresh via the supplier registry, admin |
| `timada-return` | `Return` aggregate: full-order RMA, refund via the payment provider, admin approve/reject |
| `timada-dropship` | `Supplier` trait, `SupplierRegistry`, `SupplierOrder` aggregate, built-in `MockSupplier` |
| `timada-dropship-aliexpress` | AliExpress `Supplier` adapter (stub — needs approved API credentials) |
| `timada-admin` | Umbrella admin: composes every crate's `admin_router()` behind one mountable router |
| `demo-store` (`apps/`) | Demo storefront + admin wiring the whole framework together |

## Mounting the admin in your own app

```rust
let app = axum::Router::new()
    .merge(timada_web::asset_router())
    .merge(timada_catalog::store_router(catalog_state))
    // ... other storefront routers ...
    .merge(timada_auth::admin_auth_router(auth_state.clone()))
    .nest(
        "/admin",
        timada_admin::router(admin_services).layer(
            axum::middleware::from_fn_with_state(auth_state, timada_auth::require_admin),
        ),
    );
```

`timada-auth` ships session-backed admin login (argon2 hashes, revocable
DB-backed sessions). The login router mounts *outside* the protected nest; any
other tower auth layer works in its place.

## Development

```sh
nix develop            # provides Rust, tailwindcss, sqlite, cargo-watch
./scripts/dev.sh       # migrate + serve demo-store with auto-reload
```

Or manually:

```sh
cargo run -p demo-store -- migrate apply
cargo run -p demo-store -- seed
cargo run -p demo-store -- serve
```

Seeding creates the demo admin (`admin@timada.example` / `admin` — sign in at
`/admin/login`; create or reset admins with
`cargo run -p demo-store -- create-admin --email … --password …`), two regions
(Europe in EUR with FR/DE/LU VAT rates, United States in USD), USD prices on
every other product, and the `WELCOME10` discount (10 %, first 100 uses).

Then walk the vertical slice: browse `/`, add a product to the cart, apply
`WELCOME10`, check out, watch the order at `/orders/{id}` progress
Placed → Paid → Forwarded, and use `/admin/shipping` "Refresh tracking" to
advance the mock supplier's shipment to Dispatched and then Delivered. A
product priced ending in `.99` triggers the fake payment provider's decline
path — the order compensates to Cancelled.

Some more paths to try:

- **Accounts**: register at `/register`; orders placed while signed in appear
  under `/account/orders`, and checkout prefills your email. Guest checkout
  still works.
- **Regions**: switch at `/region`. The storefront shows only products priced
  in the region's currency, the cart locks to its first line's currency, and
  each region's countries carry their own VAT rate (`/admin/regions`).
- **Returns**: once delivered, "Request a return" on the order page opens the
  RMA; approve it at `/admin/returns` to refund the charge in full and issue a
  credit note. The order itself stays Delivered — a return is a new
  conversation, not a rewrite.

Prices are tax-inclusive (EU B2C style): checkout extracts the destination
country's VAT — from the discounted amounts when a code applies — and
snapshots the net/tax split onto the order. When an order is paid, an invoice
(INV-000001, …) is issued automatically and printable at
`/orders/{id}/invoice`; a paid order that ends up cancelled, or refunded
through a return, gets a sequential credit note reversing it. See
`/admin/invoices`.

## Things to know

- **Crate names are permanent.** evento persists aggregate types as
  `"{cargo_pkg_name}/{EnumName}"` in the event log. Renaming a crate breaks
  reads of previously stored events.
- **Reads come from projections, never aggregates.** Customer-facing pages that
  need read-your-own-write (cart, order status) load evento projections through
  the `Rw` executor; admin lists read eventually-consistent SQL tables.
- **Cross-crate Askama layouts** are resolved via each crate's `askama.toml`
  (`dirs = ["templates", "../web/templates"]`). This works in-workspace but not
  under `cargo publish`; vendor the layout files per crate before publishing.
- **Migrations**: read-model tables are owned per crate via `sqlx_migrator`;
  the event-store schema is owned and migrated by evento itself.
- **No event upcasting yet.** Events are bitcode-encoded (positional, not
  self-describing), so adding a field to an *existing* event struct breaks
  replay of stored streams; new aggregates and new enum variants are safe. The
  demo `data/` directory is throwaway — after pulling a breaking change, run
  `rm -rf data && cargo run -p demo-store -- seed`. Event versioning is a
  stated pre-1.0 requirement before real data lands anywhere.
- **Credentials and counters live in SQL, not events.** Password hashes must
  be rotatable, sessions revocable, and discount redemption / invoice
  numbering need atomic counters — all deliberately write-side SQL state
  alongside the event store.
