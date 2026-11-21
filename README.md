# Timada

A multi-tenant commerce platform in Rust: a dashboard where merchants run their
shops (catalog, orders, payments through Stripe, French invoicing, mail, reviews,
newsletters, automations, pages), a store API that headless storefronts read their
shop's events from, and a storefront library with a shop to start your own from
(`storefront/`). Every bounded context is event-sourced on
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
make check       # tests, clippy, formatting, unused dependencies
make e2e         # the browser suite on three fresh servers at once (needs `make up`)
make e2e.cluster # the same suite on three nodes behind the balancer
```

The dashboard reads `config/dev.local.toml` (gitignored; start from
`config/default.toml`). To fill a shop with a real catalogue and run the storefront
against it, see `seeds/README.md` and `docs/storefront.md`.

## Where things are

| | |
|---|---|
| `crates/*` | the bounded contexts (identity, tenancy, catalog, orders, discounts, invoicing, reviews, shoppers, newsletter, automations, pages, support, aliexpress), `timada-core` for what they share, `timada-storefront` for headless shops |
| `dashboard/` | the composition root: the web app, the store API, the subscriptions, the CLI (`serve`, `migrate`, `reset`, `seed`, `snapshot`) |
| `storefront/` | a shop on the storefront library with no brand and no colours, to start your own from |
| `tests/e2e` | the Playwright suite |
| `docs/` | `architecture.md` first, then one file per area (`storefront`, `payments`, `invoicing`, `mail`, `automations`, `aliexpress`, `loadtest`) |

## Releasing

Three workflows, as in the other timayz services:

- **Builder** (by hand) pushes `timayz/timada:builder` from `builder.Dockerfile`: the
  toolchain, the Tailwind and topcoat CLIs, and a first build of the dashboard. Run it
  once before the first release, and again when the toolchain or the topcoat version
  moves (the CLI versions are pinned in that file).
- **Release** (by hand) reads the commits since the last tag, bumps the crates'
  versions, writes the GitHub release and pushes the `v…` tag.
- **Publish** (on that tag) builds `Dockerfile` on top of the builder image and pushes
  `timayz/timada:<version>`: the `timada-dashboard` binary and its asset bundle, run as
  user 10001, with `/var/lib/timada` as the place for its data.

`helm/` deploys that image: a StatefulSet with one volume a node for the store, the
pictures and the personal-data keys, the configuration from `values.yaml` (`config`,
written as the server's file) and the secrets as `TIMADA__…` variables
(`extraSecretEnv`). `replicas` is one node by default. With three, the chart writes the
`[cluster]` section itself: each pod is named to the others through a headless Service
(`timada-<i>.timada-headless`), its ordinal is its `node_id`, and the nodes talk over
mutual TLS, each pod with a certificate of its own from cert-manager's CSI driver
(`clusterCa`). The pods start together and are
updated one at a time, each serving a few seconds more (`preStopSeconds`) while the
Service stops sending it requests. A cluster takes a node more while it runs: `founders`
says how many it started with, the new pod waits over its empty volume, and a platform
administrator adds it on the cluster page (`values.yaml` has the steps). One cluster may also run over
several Kubernetes clusters, a release in each: `peers` then names every node as the others
reach it (on 7000 for the store, 7001 for the rest), `firstNodeId` numbers this
release's pods among them, and `clusterCa.secretName` gives them all one authority. `zones`
names the zones to run in instead, one node in each. Each pod takes its own name as its
`server.work_group`, so the background work on what a node created is done by that node
alone; `config.server.work_group` gives them all the same one.

Every change goes through a branch and a pull request; `make check` must be green
before one is opened.

## License

Timada is source-available, in two parts:

- **The libraries and the shop to start from** (`crates/`, `storefront/`) are under
  [MIT](crates/LICENSE-MIT) or [Apache 2.0](crates/LICENSE-APACHE), as you prefer: a
  shop built on them is yours.
- **Everything else** (the dashboard, the chart, the tests and the tooling) is under
  the [Business Source License 1.1](LICENSE). You may read it, change it, and run it
  in production for shops you own, as many as you like. Offering it to others as a
  hosted or managed service needs a commercial license. Four years after a version
  is published, that version becomes AGPL-3.0-or-later.
