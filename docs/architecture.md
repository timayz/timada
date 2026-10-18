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
| `crates/catalog-import` | `timada-catalog-import` | Fills a catalog from a snapshot file (`seeds/`), and takes one from a WooCommerce shop (`woo-snapshot`) |
| `crates/orders` | `timada-orders` | Carts and the orders placed from them, next to the catalog in a tenant's store |
| `crates/discounts` | `timada-discounts` | Discount codes and how often each was used, and the checkout offers, in a tenant's store |
| `crates/reviews` | `timada-reviews` | What buyers wrote about the products of their orders, and whether the merchant published it |
| `crates/shoppers` | `timada-shoppers` | A shop's customers who sign in: mailed codes, sessions, saved addresses, in a tenant's store |
| `crates/support` | `timada-support` | The messages visitors send through a shop's contact form, and whether the merchant handled them |
| `crates/invoicing` | `timada-invoicing` | The invoices and credit notes of the orders, numbered under French rules, drawn as PDF (see `docs/invoicing.md`) |
| `crates/storefront` | `timada-storefront` | The library a merchant app depends on (see `docs/storefront.md`) |
| `crates/core` | `timada-core` | Reserved; holds no code yet |
| `dashboard` | `timada-dashboard` | The composition root: runtime, HTTP, auth, store API, CLI |
| `examples/storefront` | `storefront-example` | A minimal merchant app on the library, driven by the browser suite |
| `examples/chatounette` | `chatounette-example` | A French cat shop in chatounette.com's layout and colours on the library, fed by `seeds/chatounette.json.gz` |
| `tests/e2e` | | Playwright suite against the running dashboard |
| `events.lock` | | Every persisted shape, frozen (see *Evolving events*) |

Dependencies point one way: context crates depend on evento and sqlx only (plus
Typst in `timada-invoicing` behind its `pdf` feature,
`timada-catalog` from the storefront library, from `timada-orders`, which prices
an order from the products it loads, and from `timada-discounts`, for its currencies
and amounts; `timada-reviews` depends on `timada-orders`, to check that a reviewer
bought what they review, and `timada-shoppers` on it, for the shape of an address and
the rule an e-mail is kept under); the dashboard depends on every crate;
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
- **One stream per tenant for a short list.** The settings a checkout reads whole each
  time (shipping methods, checkout offers) live in one stream with a deterministic id
  (`hash_ids(["offers"])`), capped in size, and are read straight from it: no read
  model, and a change shows at once.
- **A price that changes goes into the lines.** A checkout offer prices one unit of a
  line: the quote splits that line in two (the unit at the offer's price, the rest at
  full price), so `PaymentRequested`, `Placed`, the invoice and the mail carry it
  without a new field. A discount code, by contrast, stays beside the lines at full
  price (`Discounted`).

## Where data lives

```
data/timada.db            the platform store: identity + tenancy events, every read-model
                          table (including the tenants' catalog_products,
                          catalog_categories, orders_orders, orders_carts and
                          reviews_reviews), the consensus journal
data/tenants/<id>.db      one per tenant: its catalog, cart, order and review events and
                          its consensus journal
data/media/<id>/          one per tenant: the product pictures and the review photos,
                          named <image id>.<ext>
data/shopper-keys/<id>/   one per tenant: each shopper account's and each contact message's
                          key, sealed, named <key id> (<key id>.gone once destroyed)
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
   arriving from its peers, and leaves it open for the eviction to manage. A node whose
   tenant file is gone (a new disk, a deleted file) cannot get back what its peers
   already tidied up that way: `open` copies the tenant from a peer instead
   (`needs_join`, a `.joining` marker next to the file while it runs). The copy
   arrives page by page (4 MiB at a time), so a tenant of any size comes back; a
   copy that fails halfway is discarded whole, since the marker stays and the next
   open starts the copy over from an empty file. Every finished
   write is kept on the other nodes until the node that missed it has it, so a node
   that stays down makes the others keep everything, up to `cluster.write_backlog`
   writes per tenant, past which the tenant refuses writes: a page then shows that
   changes are temporarily not accepted (the `writes_paused` code of
   `timada_core::write_refusal`, which every context's `is_user_facing`/`code`
   recognise), the store HTTP endpoints answer 503 with that code, and the store
   API carries it as a refusal a storefront shows the same way. A node not heard from on a
   tenant for `cluster.absent_after_secs` (an hour by default) is therefore given up:
   the others tidy up without it, and when its copy of the tenant reopens, they tell it
   it fell behind; `rejoin_from_peers` then closes that copy, whatever holds it,
   discards the file and makes the same copy a lost file gets. A node back before
   anything was tidied up past it is simply waited for again. The admin page of a
   tenant shows the writes it keeps and how each other node stands; `watch_backlog`
   warns in the log at four fifths of the limit and whenever a node is given up. The
   list of per-tenant read models is `tenant_subscriptions()` in `serve.rs`; adding a
   tenant-scoped context means adding one entry there.
3. **Store API** (`store_api.rs`). An evento-remote listener. Each connection opens
   with an API key token; the resolver loads the key by the token's hash, leases the
   tenant (a leased tenant is never evicted) and serves the connection with that
   tenant's executor and nothing else. A key without the `store:write` scope gets that
   executor wrapped by `read_only_store.rs`: appends refused, snapshot saves dropped,
   subscriber cursors kept under the key's own name. A key holding `store:checkout`
   (what a storefront that sells needs) gets the same wrapper with one opening: it
   may append what a shopper does, filling a cart and asking to pay for it
   (`timada_orders::is_shopper_event`), and nothing that belongs to the merchant:
   the catalog, the answer to a request to pay, an order (one exists only once
   its payment is confirmed to the dashboard). Scopes are strings recorded by the
   `ApiKey::Scoped` event (keys older than it hold read + write). Revoking the key or
   changing its scopes ends its connections; suspending the tenant closes its
   database, and reconnects are refused.
4. **Payments** (`payments/`). Each tenant connects its own Stripe account on
   `/t/{slug}/payments`; the secret key and the webhook signing secret are recorded
   in the tenant's platform events sealed with AES-256-GCM (`seal.rs`) under the
   operator's `[payments] secret_key`, or else `[secrets] key` (no section, no
   payments). A storefront asks to pay
   (`Cart::CheckoutRequested`); the per-tenant subscription `timada-dashboard/payments`
   prices the cart from the store, opens a PaymentIntent with the tenant's key (the
   request id is the idempotency key, so every node answering opens one payment) and
   records it (`Cart::PaymentRequested`) or the refusal. The shopper confirms with
   Stripe's card form; Stripe calls `POST /stripe/webhooks/{tenant_id}`, any node,
   which checks the signature, reads the payment back from Stripe and only then
   creates the order (`place_paid_order`). A second payment of a cart is refunded;
   cancelling an order refunds it first. Setting it up and testing it locally:
   `docs/payments.md`.
5. **Shop mail** (`shop_mail.rs`). A shop's mails to its shoppers (order
   confirmations, review requests, sign-in codes) go out through the merchant's own
   SMTP server once it set one up on `/t/{slug}/mail`, and through the platform's
   `[mail]` server under the shop's name otherwise. Saving sends a test mail first.
   The password is recorded in the tenant's platform events (`MailServerConfigured`)
   sealed under `[secrets] key`; secrets never enter the tenant store, which
   replicates to storefronts. Hosts on this machine or a private network are refused
   unless `mail.allow_private_hosts` is set. Account codes and invitations always go
   through the platform's server. Each shop mail may be worded per language: the
   shopper's language comes from the storefront's `Accept-Language` and is kept on
   the order (`LanguageGiven`/`LanguageSet`). Details: `docs/mail.md`.
6. **Authorisation** (`authz.rs`). Casbin RBAC with the tenant as domain and its custom
   roles as roles. Policies are not loaded at startup: every tenant-scoped request loads
   the tenant's strict state, whose version says which events it reflects, and the
   enforcer re-syncs a tenant's rules when a newer state comes along. Owners bypass.
   `manage` on a resource implies `read`.
7. **Auth glue** (`auth/`). The web side of the identity context: OIDC discovery and
   token exchange, WebAuthn ceremonies, mailed one-time codes, the private cookies that
   carry a sign-in attempt between steps, and the session cookie.
8. **Pages** (`app/`). The topcoat router; the module tree is the URL tree.
   `/auth/*` (sign in, the account page, passkeys, e-mail, password, sessions),
   `/tenants/new` and `/t/{slug}` (members, roles, API keys, products, categories,
   orders, open carts, discounts, checkout offers, reviews, messages, payments), `/stripe/webhooks/{tenant_id}`,
   `/store/reviews` (a storefront handing in a review, see *Reviews*), `/store/shoppers/*`
   (a storefront signing its shoppers in, see *Shopper accounts*), `/store/contact` (a
   storefront handing in a message from its contact form, see *Support messages*), `/invitations/{token}`,
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

## Reviews

Only a buyer reviews: a review is written from a paid order, for a product on it, and
its id derives from the two (`hash_ids(["review", order, product])`), so there is one
per product and order. It is signed with the name the order was shipped to (`Ada L.`)
and waits until the merchant publishes it on `/t/{slug}/reviews`; a rejected one is
gone for good, and so are its photos. The rating a product shows counts published
reviews only (`reviews_product_ratings`, recounted by the read-model handler).

A storefront hands a review in over HTTP, not through the store API:
`POST /store/reviews`, multipart, with its key as a bearer token (it must allow
`store:checkout`). The photos come with it, and the dashboard treats them like product
pictures: it checks what the bytes are, stores and replicates them to a majority, and
only then records the review, deleting the files when that fails. Recording on the
dashboard's side also means no key can append a review, or describe photos that are
not there, over the store API; the review events are not shopper events there. The
storefront vouches that the shopper holds the order: an order id is not guessable, and a
storefront offers the form only on an order its own shopper placed. The media
subscription keeps review photos on every node like product pictures, and deletes them
when a review is rejected or a photo is taken off.

Some days after an order is fulfilled (7 unless the merchant picks 0 to 60, or turns
it off, on the Reviews page: the `ReviewRequests` stream), its buyer is mailed under
the shop's name with a link to `{storefront}/reviews/{token}`. Nothing in the event
store fires later, so every node runs a timer (`[reviews] request_check_secs`) that
looks in the shared read model for orders fulfilled in the last 63 days
(`orders_orders.fulfilled_at`) and mails those due within the last 3 days: a node
that was down catches up, and a shop that turns the mails on does not mail its
history. The first node to write an order's `ReviewRequest` stream
(`original_version(0)`) sends. Nothing is claimed while the shop has no storefront
address, or once every product of the order is reviewed. The token is 32 random bytes
whose hash names a `ReviewLink` stream (valid 60 days), so a storefront key reading
the store learns nothing it can use. The storefront resolves the token from the
store, remembers the order as if its own browser had placed it, and sends the shopper
to the order's review forms.

## Shopper accounts

A shopper account belongs to one shop: it lives in the tenant's store
(`timada-shoppers`), so the same address at two shops is two accounts, and the platform
users (`timada-identity`) are only the merchants' staff. There is no sign-up: a shopper
asks for a six-digit code, and the first code they hand back creates the account. The
`Shopper` stream's id derives from the normalized address, so it is its own uniqueness
index; `ShopperChallenge` keeps only the code's hash and counts the wrong guesses (five
kill it, it lives ten minutes, another may be mailed a minute later); a
`ShopperSession`'s id derives from a random token the shopper's browser keeps, so
reading the store reveals no token. An account also keeps up to ten addresses.
"Sign out everywhere else" is a cutoff on the `Shopper` (`SessionsRevoked`, as on an
identity `User`). Deleting an account appends `Forgotten`: every session is void and
the Customers row goes, while the orders stay with the shop; a later sign-in with the
address registers it again on the same stream, as a fresh account.

Events cannot be erased, so they never hold a shopper's personal data in clear
(crypto-shredding). Each registration draws a random 256-bit key: `Registered` carries
the address signed in with and `AddressSaved` the shipping address, both AES-256-GCM
under that key and bound to the shopper, the key and the field; a saved address's id
is an HMAC of its fields under the key, so it says nothing once the key is gone.
`Registered` and `Forgotten` name the key by a random id; the key itself lives outside
the store, behind `timada_shoppers::ShopperKeys`. `Forgotten` destroys it, which leaves
what the account recorded unreadable in every copy of the events, the consensus journals
included. What remains are ids: the `Shopper` and `ShopperChallenge` stream ids are
plain hashes of the address, which anyone holding a candidate address can test (the
shop's orders keep the address anyway).

The sealing itself (`timada_core::keys`: a random 256-bit key, AES-GCM, the ciphertext
bound to the owner id, the key id and the field's name) and the `Keys` trait the
contexts ask for keys through are shared by every context that records personal data;
the support messages use them too (below). The dashboard keeps the keys in
`data_keys.dir/<tenant>/<key id>`, each sealed with `[secrets] key` (no section, no
shopper accounts and no contact messages), and like a picture on a majority of the
cluster before `Registered` commits (`PUT /cluster/keys/<tenant>/<key id>`, with
`cluster.secret`). Nothing serves these files to the outside: a node lacking a key asks
its peers on the same route with a `GET` carrying the secret. Each node follows the
tenants' `Shopper` streams (`timada-dashboard/shopper-keys`) to fetch the keys of new
accounts and destroy those of deleted ones. Destroying writes a `<key id>.gone`
tombstone before removing the file, so a copy or a fetch still under way cannot bring a
key back. The read model opens the address for the Customers list with the key, and
retries until a key on its way from a peer arrives. A backup of this directory brings
deleted accounts back when restored, so keep it short-lived or skip it; losing it on
every node loses every account's data.

A storefront signs its shoppers in over HTTP, not through the store API, under
`/store/shoppers/*` with its key as a bearer token (it must allow `store:checkout`):
`code` mails the code from the shop's name, `session` checks it and answers the token,
`sign-out`, `sign-out-everywhere`, `delete`, `addresses` and `addresses/remove` act for
the shopper whose token is in `X-Shopper-Token`. None of the shopper events are shopper events on the store API, so a
key cannot sign anyone in by appending them. A storefront resolves a token with
`GET /store/shoppers/me`, which answers the account opened with its key: the store API
only ever hands out the sealed events.

A shopper's orders are the shop's orders placed with their address, guest ones
included: owning the mailbox is what entitles them to it. `GET /store/shoppers/orders`
lists them from `orders_orders` (indexed on the tenant and the e-mail), so an order paid
a moment ago may show a second later.

## Support messages

A shop's contact form writes a `Message` stream in the tenant's store
(`timada-support`): who wrote (first and last name, the address to answer at), what,
and in which language the browser browsed (the primary `Accept-Language` subtag), so
the merchant answers in it. The id is a fresh ULID. The merchant reads the messages
on `/t/{slug}/messages` (`support:read`, from the `support_messages` read model),
answers from their own mailbox (the row links `mailto:` the address; the platform
sends no mail here) and marks the message handled (`support:manage`); a message
marked too soon is reopened.

The name, address and text are personal data, and events cannot be erased: like a
shopper account, each message draws a random key and `Received` holds the four fields
sealed under it (bound to the message id, the key id and the field's name; the
language stays in clear). The key lives in the same vault as the shoppers' keys, put on
a majority of the cluster before `Received` commits, and each node follows the
`Message` streams (`timada-dashboard/message-keys`) to fetch new keys and destroy
those of erased messages. `Forgotten` (`POST /t/{slug}/messages/forget`,
`support:manage`, the "Erase" button of the row) records the erasure a sender asked
for and destroys the key: the read model drops the row, and the message answers "not
found" to every command from then on. The read model opens each message with its key
for the inbox, and retries until a key on its way from a peer arrives.

A storefront hands a message in over HTTP, not through the store API:
`POST /store/contact`, form-encoded, with its key as a bearer token (it must allow
`store:checkout`). Anyone may fill a contact form, so the dashboard counts: each key,
and each visitor at a shop (the address the storefront reports in `X-Shopper-IP`,
else the caller's), may hand in only so many messages within `[store]`'s window
(`contact_messages_per_ip`, `contact_messages_per_key`,
`contact_messages_window_secs`), past which it answers 429 `too_many_requests` and
records nothing. The limiter is the one the sign-in codes use, in this node's
memory, counted apart from the codes.

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
  and commit the lock. A shape never deployed anywhere may still change in place:
  `EVENTO_LOCK=force` rewrites the lock to the code.
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
  (three nodes in one process: a node that missed writes catches up, a lost tenant
  file is copied back, also one larger than a copy page, a node away too long is
  given up and copies the tenant back on return, a tenant at its write backlog
  refuses writes), `store_api` (TCP, keys, TLS),
  `storefront` (the library against a live store), `events_lock`. The review endpoint is
  covered over HTTP by the browser suite.
- `make e2e`: `scripts/e2e.sh` builds the dashboard and the example storefront, resets
  the database, starts the server on `config/e2e.toml` and runs Playwright through the
  compose stack (`make up`: Traefik for HTTPS, dex as an OIDC provider, MailDev as the
  mailbox the tests read codes from). Pass a spec path or a title filter to run part of it.
- `make check` runs tests, clippy with warnings denied, rustfmt and cargo-machete.
