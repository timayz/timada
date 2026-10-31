# Timada

A multi-tenant commerce platform in Rust: a dashboard where merchants run their
shops (catalog, orders, payments through Stripe, French invoicing, mail, reviews,
newsletters, automations, pages), a store API that headless storefronts read their
shop's events from, and a storefront library with a worked example
(`examples/chatounette`). Every bounded context is event-sourced on
[evento](https://crates.io/crates/evento); the web layer is
[topcoat](https://crates.io/crates/topcoat).

Nothing has been deployed yet: shapes and schemas change in place until the first
release.

## Working on it

The dev shell (`nix develop`, or direnv) holds the toolchain, Playwright, the Stripe
CLI, Tailwind and mkcert. Inside it:

```sh
make up          # Traefik, dex, MailDev and stripe-mock (compose)
make cert        # once: local certificates for *.localhost
make dev         # the dashboard at https://timada.localhost, rebuilt on change
make check       # tests, clippy, formatting, unused dependencies (what CI runs)
make e2e         # the browser suite on a fresh database (needs `make up`)
```

The dashboard reads `config/dev.local.toml` (gitignored; start from
`config/default.toml`). To fill a shop with a real catalogue and run the example
storefront against it, see `seeds/README.md` and `docs/storefront.md`.

## Where things are

| | |
|---|---|
| `crates/*` | the bounded contexts (identity, tenancy, catalog, orders, discounts, invoicing, reviews, shoppers, newsletter, automations, pages, support, aliexpress), `timada-core` for what they share, `timada-storefront` for headless shops |
| `dashboard/` | the composition root: the web app, the store API, the subscriptions, the CLI (`serve`, `migrate`, `reset`, `seed`, `snapshot`) |
| `examples/chatounette` | a French cat shop on the storefront library |
| `tests/e2e` | the Playwright suite |
| `docs/` | `architecture.md` first, then one file per area (`storefront`, `payments`, `invoicing`, `mail`, `automations`, `aliexpress`, `loadtest`) |

Every change goes through a branch and a pull request; `make check` must be green
before one is opened.
