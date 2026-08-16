# Timada

A Rust framework for building customizable e-commerce stores. Products are
sourced from pluggable **providers** — your own inventory or dropshipping
suppliers like AliExpress — and managed through a **drop-in admin UI** that
mounts into any [axum](https://github.com/tokio-rs/axum) app with one
`.nest()` call. Storefronts stay fully custom per store: the framework
provides the domain backend and the admin, nothing else.

Built on event sourcing/CQRS with [evento](https://github.com/timayz/evento),
SQLite (sqlx), Askama templates, TwinSpark, and Tailwind CSS v4.

## Workspace

| Crate | What it is |
|---|---|
| `crates/timada` | Core domain: aggregates (`Product`, `Inventory`, `ProviderConnection`), commands, read models, SQLite pools, migrations. ⚠️ The package name is part of every persisted event's type — never rename it. |
| `crates/timada-provider` | The `Provider` trait (catalog sourcing, stock, fulfillment), DTOs, and the `ProviderRegistry`. |
| `crates/timada-provider-aliexpress` | AliExpress provider (currently a deterministic stub behind the real trait). |
| `crates/timada-admin` | The mountable admin: catalog management, provider connections, product import. Assets are embedded — no asset pipeline needed in the host app. |
| `demo-store` | Example storefront binary showing the full wiring. |

## Mounting the admin

```rust
let admin = timada_admin::AdminContext::new(executor, read_pool, providers);
let app = axum::Router::new()
    .nest("/admin", timada_admin::router(admin).layer(my_auth_layer));
```

The admin ships **no authentication** — wrap the router with your own
middleware. If you mount somewhere other than `/admin`, set the prefix with
`AdminContext::with_base_path` so templates emit correct URLs.

## Providers

A provider implements `timada_provider::Provider`: search/fetch products,
query stock, and (later milestone) fulfill orders. Implementations are
registered once at startup; *connections* (credentials, enabled state) are
runtime data managed in the admin. Credentials live only in the event store —
they are never copied into read-model tables. Note that the event log is
append-only and unencrypted: for production, encrypt credential payloads at
the application level before they are committed.

```rust
let providers = Arc::new(
    ProviderRegistry::default()
        .register(Arc::new(SelfInventory::new(read_pool.clone())))
        .register(Arc::new(AliExpress)),
);
```

## Running the demo store

```sh
cargo run -p demo-store        # http://localhost:3000 (admin at /admin)
```

Uses `DATABASE_URL` (default `sqlite:timada.db?mode=rwc`). Migrations run
automatically at startup: evento owns the event-store schema, `sqlx_migrator`
owns the read-model tables.

## Development

```sh
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo machete
```

Compiled CSS is committed so the crates stay drop-in. After editing templates,
regenerate with the Tailwind v4 CLI (provided by the Nix devshell):

```sh
tailwindcss -i crates/timada-admin/assets/input.css -o crates/timada-admin/assets/admin.css --minify
tailwindcss -i demo-store/assets/input.css -o demo-store/assets/store.css --minify
```
