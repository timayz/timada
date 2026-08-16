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
| `timada-catalog` | Products: import from supplier, publish, storefront + admin |
| `timada-cart` | Guest shopping cart (cookie-based), TwinSpark fragments |
| `timada-order` | Checkout, `Order` aggregate, the order-fulfillment saga, customer order status, admin |
| `timada-payment` | `PaymentProvider` trait, `Payment` aggregate, built-in `FakePaymentProvider` |
| `timada-tax` | `TaxCalculator` trait, built-in `FixedRateVat` (tax-inclusive, per-country rates) |
| `timada-invoice` | `Invoice` aggregate: sequential numbering, credit notes on refund, printable HTML documents |
| `timada-shipping` | `Shipment` aggregate, tracking refresh via the supplier registry, admin |
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
    .nest(
        "/admin",
        timada_admin::router(admin_services)
            .layer(axum::middleware::from_fn(my_auth_middleware)),
    );
```

Authentication is yours: wrap the admin router in whatever tower layer you use.
The demo store ships a trivial example middleware.

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

Then walk the vertical slice: browse `/`, add a product to the cart, check out,
watch the order at `/orders/{id}` progress Placed → Paid → Forwarded, and use
`/admin/shipping` "Refresh tracking" to advance the mock supplier's shipment to
Dispatched and then Delivered. A product priced ending in `.99` triggers the
fake payment provider's decline path — the order compensates to Cancelled.

Prices are tax-inclusive (EU B2C style): checkout extracts the destination
country's VAT and snapshots the net/tax split onto the order. When an order is
paid, an invoice (INV-000001, …) is issued automatically and printable at
`/orders/{id}/invoice`; a paid order that ends up cancelled gets a sequential
credit note reversing it. See `/admin/invoices`.

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
