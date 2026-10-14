# Architecture

Timada is a multi-tenant e-commerce back office with a headless storefront library.
A merchant (a *tenant*) signs in to the dashboard, invites a team, manages a catalog
and mints API keys; the merchant's own web app depends on `timada-storefront`,
connects with a key and sells from that catalog in its own markup.

Everything is Rust. The web layer is [topcoat](https://crates.io/crates/topcoat)
(server-rendered pages, `view!` components, cookie sessions); state is event-sourced
with [evento](https://github.com/timayz/evento) over SQLite, replicated by its Accord
consensus executor; read models are SQL tables fed by subscriptions. One binary,
`timada-dashboard`, hosts all of it.

## Workspace map

| Path | Package | Role |
|---|---|---|
| `crates/identity` | `timada-identity` | Users, sign-in methods (OIDC, passkeys, e-mail + password), sessions |
| `crates/tenancy` | `timada-tenancy` | Tenants, custom roles, members, invitations, API keys |
| `crates/catalog` | `timada-catalog` | Products, their pictures, variants and categories: the first context living in a tenant's own store |
| `crates/storefront` | `timada-storefront` | The library a merchant app depends on (see `docs/storefront.md`) |
| `crates/core` | `timada-core` | Reserved; holds no code yet |
| `dashboard` | `timada-dashboard` | The composition root: runtime, HTTP, auth, store API, CLI |
| `examples/storefront` | `storefront-example` | A minimal merchant app on the library, driven by the browser suite |
| `tests/e2e` | | Playwright suite against the running dashboard |
| `events.lock` | | Every persisted shape, frozen (see *Evolving events*) |

Dependencies point one way: context crates depend on evento and sqlx only (plus
`timada-catalog` from the storefront library); the dashboard depends on every crate;
nothing depends on the dashboard.

## Bounded contexts

A context crate is pure domain code: aggregates, commands, read-model queries. It
knows nothing about HTTP, cookies, files, consensus groups or who is allowed to do
what; the host decides permissions and hands the crate verified facts.

Each crate follows the same layout, copied from evento's `examples/bank`:
`aggregator.rs` (the `#[evento::aggregate]` enums, names pinned to
`timada-<ctx>/<Enum>` because the aggregate type is persisted), `command/` (one file
per verb on a `Command<'_, E>` that borrows the executor), `query/` (one file per
read model), `error.rs`, `value_object.rs`.

Recurring patterns:

- **Uniqueness without a unique index.** A slug, an e-mail address, a session or API
  token is an *index aggregate* with a deterministic id (`hash_ids([kind, value])`).
  Appending its first event at version 0 is an atomic create-unless-exists, and loading
  it reads the event store directly, so a lookup sees its own writes.
- **Every event says who did it** (`*_by` fields). Authorisation lives in the host.
- **Write-side state** is a strict projection with `#[evento::snapshot(none)]`, so
  commands observe the exact version they write against.
- **Read models** are SQL tables in the shared database, fed by one
  `read_model_subscription` per crate. They are eventually consistent; pages that must
  show a write immediately read the aggregate.

## Where data lives

```
data/timada.db            the platform store: identity + tenancy events, every read-model
                          table (including the tenants' catalog_products and
                          catalog_categories), the consensus journal
data/tenants/<id>.db      one per tenant: its catalog events and its consensus journal
data/media/<id>/          one per tenant: the product pictures, named <image id>.<ext>
<merchant app>/store.db   a storefront replica: subscription cursor, snapshots, product and
                          category rows
```

The platform store is opened once with two pools (`dashboard/src/db.rs`): a read
pool and a single-connection write pool, so writes serialize up front instead of
failing with `SQLITE_BUSY`. The write side runs WAL with `synchronous = FULL`.

Tenant-scoped contexts write to the tenant's file through the tenant's executor and
project into the shared database keyed by tenant id, so one dashboard query lists a
tenant's products while the events stay isolated by construction.

## The runtime (dashboard)

`dashboard/src/cli/serve.rs` wires everything, in this order.

1. **Cluster** (`cluster.rs`). One `MuxTransport`, one `SweepScheduler` and one
   `GroupHost` per process. Every event database is an Accord consensus group: the
   platform store is the default group, each tenant a group of its own. Writes are
   coordinated across the cluster (`[cluster]` peers; a majority must agree), reads,
   snapshots and subscriptions are served from this node's own files. Without a
   `[cluster]` section the node talks to itself over a loopback port, so one code
   path serves both deployments.
2. **Tenants** (`tenants.rs`). A tenant is *up* while its database is open: its
   consensus group runs and its read-model subscriptions follow its stream. Every
   tenant-scoped request goes through `Tenants::open`, which is the activity signal.
   A tenant stays open until opening another one goes past `tenants.max_open`; then
   the least recently used is evicted. Left unset, `max_open` is worked out at
   startup (`limits.rs`) from the open-file limit (raised to its hard maximum first)
   and the available memory, and logged. Requests are not the only use: a node
   taking part in writes another node coordinates sees none, so a group whose applied
   state moved since the last look counts as just used, and one with a transaction
   in flight is never evicted, nor is one held by a store connection or a request. A
   new tenant opens on every node as soon as its creation reaches it (a platform
   subscription, `creation_subscription`; creations older than a minute are history
   being replayed and are skipped), so its group already runs everywhere when the
   first write comes, typically a second after the creation. A
   peer talking about a group this node closed triggers `reopen`, which opens it only
   if the catalog knows the tenant and it is not suspended: the wire cannot create
   one. The catalog is a read model, a second behind a tenant created on another
   node, so the lookup is retried for as long as the transport keeps the frames
   waiting; nodes also wait three seconds (`recovery_timeout`) before taking over a
   transaction, so a peer that opens late joins the write instead of recovering it
   from under its coordinator. Sweep gossip never opens a tenant, so a node that was
   down would fetch the writes it missed only once something opens the tenant there:
   at startup in a cluster, `start_catch_up` waits for the platform store to settle,
   then opens every active tenant (four at a time), holds it until events stop
   arriving from its peers, and leaves it open for the eviction to manage. The list of
   per-tenant read models is `tenant_subscriptions()` in `serve.rs`; adding a
   tenant-scoped context means adding one entry there.
3. **Store API** (`store_api.rs`). An evento-remote listener. Each connection opens
   with an API key token; the resolver loads the key by the token's hash, leases the
   tenant (a leased tenant is never evicted) and serves the connection with that
   tenant's executor and nothing else. A key without the `store:write` scope gets that
   executor wrapped by `read_only_store.rs`: appends refused, snapshot saves dropped,
   subscriber cursors kept under the key's own name. Scopes are strings recorded by the
   `ApiKey::Scoped` event (keys older than it hold read + write). Revoking the key or
   changing its scopes ends its connections; suspending the tenant closes its
   database, and reconnects are refused.
4. **Authorisation** (`authz.rs`). Casbin RBAC with the tenant as domain and its custom
   roles as roles. Policies are not loaded at startup: every tenant-scoped request loads
   the tenant's strict state, whose version says which events it reflects, and the
   enforcer re-syncs a tenant's rules when a newer state comes along. Owners bypass.
   `manage` on a resource implies `read`.
5. **Auth glue** (`auth/`). The web side of the identity context: OIDC discovery and
   token exchange, WebAuthn ceremonies, mailed one-time codes, the private cookies that
   carry a sign-in attempt between steps, and the session cookie.
6. **Pages** (`app/`). The topcoat router; the module tree is the URL tree.
   `/auth/*` (sign in, the account page, passkeys, e-mail, password, sessions),
   `/tenants/new` and `/t/{slug}` (members, roles, API keys, products), `/invitations/{token}`,
   `/admin/*` (users, tenants). `ui.rs` holds the shared furniture.

Shutdown stops the HTTP server, then the store API (its leases hold tenants open), the
sweeper and the tenants, the read-model subscriptions and finally the pools.

## One write, end to end

A member renames a product on `/t/{slug}/products/{id}`:

1. The page handler loads the session, the tenant state and the enforcer's answer for
   `products:manage`.
2. `tenants.open(tenant)` returns the tenant's executor (opening the file and its
   consensus group if needed).
3. `timada_catalog::Command::rename_product` loads the product's write-side state, checks the
   invariants and commits `Renamed { name, renamed_by }` at the observed version.
4. The commit goes through the tenant's Accord group and lands in `data/tenants/<id>.db`.
5. The tenant's `catalog` subscription writes the new name into `catalog_products` in
   the platform store; the next page load shows it. The same subscription follows the
   `Category` streams into `catalog_categories`, and rewrites a product's rows in
   `catalog_product_categories` whenever it is filed under a category or leaves one.
6. Every storefront connected to that tenant through the store API is notified at
   once; its `storefront/catalog` subscription updates the local `storefront_products`
   row.

## What a product varies by

A product that comes in several colours or sizes declares its *options* and then one
*variant* per combination, each with its own price and optionally the merchant's own
code. The options settle first: once variants exist they cannot change, because a
variant names a value for every option exactly once and that has to stay true. A
product with variants is published without a price of its own, and what a listing
shows is the range across them. Every price on one product is in one currency, so a
range always means something.

Codes are unique across a tenant through the same index-aggregate pattern as slugs
(`timada-catalog/ProductSku`), compared without regard to case. A variant that goes,
and an archived product, give their codes back.

## Product pictures

The event store records what a picture is, never its bytes: an id, the format, the
size, what it shows. The file lands in `media.dir/<tenant>/<image id>.<ext>` and is
served at `/media/...` by a directory route with `Cache-Control: immutable`, since a
new picture is a new id and an address never changes what it holds. There is no
authentication on it: an id is an unguessable ULID, and a shopper's browser has no
session on this node.

`dashboard/src/media.rs` writes and deletes the files. An upload stores the file and
then commits, deleting it again if the commit fails; a removal commits and then
deletes. So a failure leaves a file nobody references rather than a product pointing
at nothing. `media.public_url` is the address a shopper loads a picture from, shown on
each tenant's API keys page for its storefront to be configured with.

With several nodes, every node keeps every picture, so a node going down takes none
with it and `media.public_url` can point at any of them or at a balancer in front.
The node taking an upload stores the file, copies it to the other nodes
(`PUT /cluster/media/<tenant>/<file>` on their `cluster.web_peers` address, with
`cluster.secret` as a bearer token) and commits only once a majority of the cluster
holds it, the same majority the commit itself needs; short of that the upload is
refused and the copies are left unreferenced. Copies to the slower nodes carry on
after the commit. Each node also runs a per-tenant subscription
(`timada-dashboard/media`) that fetches from a peer's `/media` any picture added that
it lacks (a node that was down during the upload, or a new one replaying the stream)
and deletes the pictures removed. It only runs while the tenant is open on that node,
so a node that missed a picture catches up the next time the tenant opens there.

## The storefront side

`timada-storefront` runs inside the merchant's app. Its executor is a `Sidecar`:
events go through the remote client to the store API, while the subscription cursor,
the projection snapshots and the product rows live in the app's own SQLite file. One
file per replica, so replicas never fence each other out of the subscription and
nothing accumulates on the tenant's store. The library registers no page of its own
(route discovery belongs to the host); it offers queries, a guard, one component and
`GET {prefix}/status`. Details and the example app are in `docs/storefront.md`.

## Evolving events

Events are encoded with bitcode, which is positional: a stored event's layout, and
every type nested in it, is frozen. `events.lock` at the workspace root records every
event, every encoded type and every snapshotted view; `dashboard/tests/events_lock.rs`
fails when a frozen line changes. The rules:

- Never edit or remove a persisted variant, field or enum. Add a variant, a companion
  event, or a new event with `#[evento(upcast_to = New)]` on the old one.
- After adding a shape: `EVENTO_LOCK=update cargo test -p timada-dashboard --test events_lock`
  and commit the lock.
- A snapshotted view whose shape changes bumps `.revision(n)`; its name is pinned
  with `name = "timada-<ctx>/<View>"` so a move does not orphan snapshots.

## Configuration

`config/default.toml` is the full shape with its defaults; a file given with
`--config` (or `TIMADA_CONFIG`) overrides it, and `TIMADA__<SECTION>__<KEY>`
environment variables override both. Sections: `server` (URL, bind, region, trusted
proxies), `database`, `auth` (secret, session lifetime, OIDC providers), `mail`,
`tenants`, `store_api` (omit to not expose it), `cluster` (omit for a single node),
`monitoring`. `config/dev.local.toml` is the developer's own file and is ignored by
git; `config/e2e.toml` is what the browser suite runs on. Configuration is written for
operators: it names what things do, not the libraries behind them.

## Testing

- `cargo test --workspace` (inside `nix develop`): the crates' domain tests over a
  temporary store, and the dashboard's integration tests, which boot the runtime in
  process: `tenants_runtime` (open, evict, reopen, catch-up), `tenants_cluster`
  (three nodes in one process: a node that missed writes catches up), `store_api` (TCP, keys, TLS),
  `storefront` (the library against a live store), `events_lock`.
- `make e2e`: `scripts/e2e.sh` builds the dashboard and the example storefront, resets
  the database, starts the server on `config/e2e.toml` and runs Playwright through the
  compose stack (`make up`: Traefik for HTTPS, dex as an OIDC provider, MailDev as the
  mailbox the tests read codes from). Pass a spec path or a title filter to run part of it.
- `make check` runs tests, clippy with warnings denied, rustfmt and cargo-machete.
