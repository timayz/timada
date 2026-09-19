# timada-admin

A [topcoat](https://github.com/tokio-rs/topcoat) admin for the timada contexts
(orders, products, stock, customers, promotions) that you mount into your own app — a topcoat app
or any tower/axum app.

The admin is a self-contained topcoat `Router`: its own layout, session, auth
layer and app context. It never touches events directly; every read goes
through the contexts' projections and SQL read models, every write through
their `Command`s.

## Mounting

```rust
use timada_admin::{AdminConfig, AdminServices};
use topcoat::asset::AssetBundle;

let bundle = AssetBundle::load()?;            // `topcoat asset bundle --bin <your-bin>`
let services = AdminServices::new(executor, pool);   // any `evento::Executor` + the read-model pool

// topcoat host
let router = timada_admin::mount(
    topcoat::router::Router::builder().discover().assets(bundle.clone()),
    AdminConfig::default(),                   // mount: "admin", stylesheet: Bundled
    bundle,
    services,
).build();

// axum host (feature `axum`)
let app = timada_admin::mount_axum(axum::Router::new(), AdminConfig::default(), bundle, services);
```

Both mount `/admin` and `/admin/{*rest}` **without stripping the prefix**:
the admin registers its routes under the real segment (renamed at runtime
from `AdminConfig::mount`, one segment only), so every href, redirect and
asset URL it generates is correct. topcoat documents that a prefix-stripping
mount (axum `nest_service`) breaks generated URLs — don't use one.

Run the admin's migrations next to the contexts' (`timada_admin::migrations()`),
create an operator with `create_admin(&pool, email, password)`, then sign in
at `/admin/login`. Sessions use a dedicated `__Host-timada_admin` cookie.

The listings read the contexts' SQL read models, so the host must run their
subscriptions: `timada_order::order_history_subscription`,
`timada_catalog::product_list_subscription`,
`timada_inventory::stock_list_subscription`,
`timada_customer::customer_list_subscription` and
`timada_promotion::code_list_subscription`, each with `.data(pool)`.

## Assets and styling

The Tailwind stylesheet is built by this crate's `build.rs` (its scan is
rooted here, which is why the host cannot build it) and bundled with the host
binary by `topcoat asset bundle` / `topcoat dev`. Set `TAILWIND_CLI` to a
local `tailwindcss` binary to avoid the download. Without a bundle, pass
`Stylesheet::Url(..)` to keep the admin usable (unstyled).

The UI components in `src/components` are vendored from topcoat-ui and
exported (`timada_admin::components`) for hosts that extend the admin.

## Caveats

- The host must not call `module_router!()`: topcoat 0.8.1's module
  discovery panics on module-derived handlers outside its root, and the
  admin's are. `Router::builder().discover()` and explicit-path pages are fine
  — the admin walks the inventory itself, filtered by its own root, so the
  host's `path_param!` segments and module-derived handlers don't affect it.
- `Segment::new` is `#[doc(hidden)]` in topcoat 0.8.1; the crate pins that
  exact version.
- `AdminConfig::mount` is a single path segment.
