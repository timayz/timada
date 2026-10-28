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
| `crates/catalog` | `timada-catalog` | Products, their pictures, variants and categories: the first context living in a tenant's own streams |
| `crates/catalog-import` | `timada-catalog-import` | Fills a catalog from a snapshot directory (`seeds/`, catalogue + pictures), and takes one from a WooCommerce shop (`woo-snapshot`) |
| `crates/aliexpress` | `timada-aliexpress` | The items a shop imports from AliExpress through the platform's app and its company's buyer account: the signed open-platform client, the mapping to a draft, what a read changes, one link stream per item in a tenant's store (see `docs/aliexpress.md`) |
| `crates/orders` | `timada-orders` | Carts and the orders placed from them, next to the catalog in a tenant's store |
| `crates/discounts` | `timada-discounts` | Discount codes and how often each was used, and the checkout offers, in a tenant's store |
| `crates/reviews` | `timada-reviews` | What buyers wrote about the products of their orders, and whether the merchant published it |
| `crates/shoppers` | `timada-shoppers` | A shop's customers who sign in: mailed codes, sessions, saved addresses, in a tenant's store |
| `crates/support` | `timada-support` | The messages visitors send through a shop's contact form, and whether the merchant handled them |
| `crates/newsletter` | `timada-newsletter` | The addresses that asked for a shop's news, confirmed through a mailed link (double opt-in) |
| `crates/automations` | `timada-automations` | The scenarios a shop's team draws: a trigger, then a tree of steps, kept as a definition the editor works on (see `docs/automations.md`) |
| `crates/invoicing` | `timada-invoicing` | The invoices and credit notes of the orders, numbered under French rules, drawn as PDF (see `docs/invoicing.md`) |
| `crates/storefront` | `timada-storefront` | The library a merchant app depends on (see `docs/storefront.md`) |
| `crates/core` | `timada-core` | Reserved; holds no code yet |
| `dashboard` | `timada-dashboard` | The composition root: runtime, HTTP, auth, store API, CLI |
| `examples/storefront` | `storefront-example` | A minimal merchant app on the library, driven by the browser suite |
| `examples/chatounette` | `chatounette-example` | A French cat shop in chatounette.com's layout and colours on the library, fed by the `seeds/chatounette` snapshot |
| `tests/e2e` | | Playwright suite against the running dashboard |
| `events.lock` | | Every persisted shape, frozen (see *Evolving events*) |

Dependencies point one way: context crates depend on evento and sqlx only (plus
Typst in `timada-invoicing` behind its `pdf` feature,
`timada-catalog` from the storefront library, from `timada-orders`, which prices
an order from the products it loads, and from `timada-discounts`, for its currencies
and amounts; `timada-reviews` depends on `timada-orders`, to check that a reviewer
bought what they review, and `timada-shoppers` on it, for the shape of an address and
the rule an e-mail is kept under; `timada-aliexpress` depends on the catalog, the
import and tenancy, for what it fills and the shop's price rule, and on reqwest, the
one context crate that calls out); the dashboard depends on every crate;
nothing depends on the dashboard.

## Bounded contexts

A context crate is pure domain code: aggregates, commands, read-model queries. It
knows nothing about HTTP, cookies, files, consensus or who is allowed to do
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
data/store.db             the store: every tenant's events (the platform's own in the
                          default tenant ""), the snapshots, the subscription cursors
                          and the consensus journal; what the cluster replicates
data/read-models.db       every read-model table (tenancy_tenants, catalog_products,
                          orders_orders, reviews_reviews, ...), each row carrying its
                          tenant id; derived from the store, rebuilt from it when lost
data/media/<id>/          one per tenant: the product pictures and the review photos,
                          named <image id>.<ext>
data/shopper-keys/<id>/   one per tenant: each shopper account's and each contact message's
                          key, sealed, named <key id> (<key id>.gone once destroyed)
<merchant app>/store.db   a storefront replica: subscription cursor, snapshots, product and
                          category rows
```

Each file is opened with a read pool and single-connection write pools
(`dashboard/src/db.rs`), so writes serialize up front instead of failing with
`SQLITE_BUSY`. The store has two write connections with different promises
(`store_writer.rs`): the events the node applies and the consensus journal share a
`synchronous = FULL` one, so an apply is on disk before the journal records it; the
subscription cursors and the snapshots go through a `synchronous = NORMAL` one, since
a cursor lost to a power cut only makes its subscription redo a few events and a
snapshot is a cache. Twenty-odd subscriptions acknowledge every event they take: with
an fsync apiece on the connection the journal flushes on, the node fell behind its
own timeouts under load and answered writes with spurious version conflicts.

A stream is identified by its tenant, its aggregate type and its id: the same product
id in two tenants is two streams. Tenant-scoped contexts carry the tenant in their
`Command` gateway (`timada_catalog::Command::new(store, tenant_id)`) and name it on
every write, load and read (`.tenant(..)` on evento's builders); the platform contexts
(identity, tenancy) live in the default tenant and keep the plain `Command(store)`. One
subscription per read model follows every tenant's streams and stamps each row with
`event.tenant`, so one query lists a tenant's products while the events stay apart
by construction.

## The runtime (dashboard)

`dashboard/src/cli/serve.rs` wires everything, in this order.

1. **Cluster** (`cluster.rs`). One Accord node per process over the store, wired
   like evento's `bank-axum-accord` example: the node, its inbox loop and its
   recovery sweep, over `TcpTransport` with the `[cluster]` peers (a majority must
   agree on every write) or an in-memory transport alone. Reads, snapshots and
   subscriptions are served from this node's own file. With several nodes a
   single-aggregate read waits for the writes a quorum already committed
   (`linearizable_reads`), so a page that reads what it just wrote sees it whichever
   node answers. Every finished write is kept on the other nodes until the node that
   missed it has it, so a node that stays down makes the others keep everything, up
   to `cluster.write_backlog` writes, past which this node refuses writes: a page then
   shows that changes are temporarily not accepted (the `writes_paused` code of
   `timada_core::write_refusal`, which every context's `is_user_facing`/`code`
   recognise), the store HTTP endpoints answer 503 with that code, and the store API
   carries it as a refusal a storefront shows the same way. A node not heard from for
   `cluster.absent_after_secs` (an hour by default) is therefore given up: the others
   tidy up without it, and when it returns they tell it it fell behind. It then
   refuses writes, logs the procedure and marks the store for a copy
   (`<store file>.joining`); the next `serve` empties the store (events, snapshots,
   cursors; the journal stays), recreates the read-model database and opens the node
   by joining, which installs a peer's store page by page (4 MiB at a time) before
   anything is served, then the subscriptions rebuild the read models from it. The
   `rejoin` command sets the same mark by hand, for a lost store file (delete the
   file first: an empty journal comes back under the node's own id through evento's
   amnesia fence). A node back before anything was tidied up past it is simply waited
   for again. `/admin/cluster` shows this node: the writes it keeps, how each other
   node stands, whether a copy is needed; `Cluster::watch` warns in the log at four
   fifths of the backlog and whenever a node is given up.
2. **Subscriptions** (`serve.rs`, `subscriptions()`). Every subscription the node runs
   is started once on the store and follows every tenant's streams: the platform's
   read models (users, the tenant catalog, the members' history), the tenants' read
   models into the read-model database, this node's copies of the pictures and the
   personal-data keys, the answers the dashboard writes back (payments, invoices),
   the mails it sends (order confirmations, message notices, newsletter opt-ins).
   A handler takes the tenant from `event.tenant` and builds the gateway it writes
   back with from it. Adding a tenant-scoped context means adding one entry there.
3. **Store API** (`store_api.rs`). An evento-remote listener. Each connection opens
   with an API key token; the resolver loads the key by the token's hash and serves
   the connection in the key's tenant (`Session::tenant`): every request acts in that
   tenant whatever it names, the events it reads come back as from a store of its own,
   and the subscriber calls are refused (a storefront keeps its cursors locally). A
   key without the `store:write` scope gets the store wrapped by `read_only_store.rs`:
   appends refused, snapshot saves dropped. A key holding `store:checkout`
   (what a storefront that sells needs) gets the same wrapper with one opening: it
   may append what a shopper does, filling a cart and asking to pay for it
   (`timada_orders::is_shopper_event`), and nothing that belongs to the merchant:
   the catalog, the answer to a request to pay, an order (one exists only once
   its payment is confirmed to the dashboard). Scopes are strings recorded by the
   `ApiKey::Scoped` event (keys older than it hold read + write). Revoking the key or
   changing its scopes ends its connections; suspending the tenant ends every
   connection of the tenant, and reconnects are refused.
4. **Payments** (`payments/`). Each tenant connects its own Stripe account on
   `/t/{slug}/payments`; the secret key and the webhook signing secret are recorded
   in the tenant's platform events sealed with AES-256-GCM (`seal.rs`) under the
   operator's `[payments] secret_key`, or else `[secrets] key` (no section, no
   payments). A storefront asks to pay
   (`Cart::CheckoutRequested`); the subscription `timada-dashboard/payments`
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
   through the platform's server; newsletter campaigns and their test mails never do,
   they go through the shop's own server only (`ShopMail::own_sender_of`). Each shop mail may be worded per language: the
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
   orders, open carts, discounts, checkout offers, reviews, messages, newsletter, automations, payments), `/stripe/webhooks/{tenant_id}`,
   `/store/reviews` (a storefront handing in a review, see *Reviews*), `/store/shoppers/*`
   (a storefront signing its shoppers in, see *Shopper accounts*), `/store/contact` (a
   storefront handing in a message from its contact form, see *Support messages*),
   `/store/newsletter/*` (sign-ups and their links, see *Newsletter*), `/invitations/{token}`,
   `/admin/*` (users, tenants). `ui.rs` holds the shared furniture.

Shutdown stops the HTTP server, then the store API, the sweeps, the subscriptions,
the node and finally the pools.

## One write, end to end

A member renames a product on `/t/{slug}/products/{id}`:

1. The page handler loads the session, the tenant state and the enforcer's answer for
   `products:manage`.
2. `timada_catalog::Command::new(store, tenant_id)` is the catalog gateway in the
   tenant's streams.
3. `rename_product` loads the product's write-side state in that tenant, checks the
   invariants and commits `Renamed { name, renamed_by }` at the observed version.
4. The commit goes through the Accord node and lands in `data/store.db`, in the
   tenant's stream of that product.
5. The `catalog` subscription writes the new name into `catalog_products` in the
   read-model database, under the event's tenant; the next page load shows it. The
   same subscription follows the `Category` streams into `catalog_categories`, and
   rewrites a product's rows in `catalog_product_categories` whenever it is filed
   under a category or leaves one.
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

## Stock

A shop may count what it holds of a product without variants, or of each variant.
Each counted thing is its own stream (`timada-catalog/StockItem`, deterministic id
from the product and variant), so a product's stream stays short however often it
sells. A thing without a stream, or untracked, sells without limit: counting is
opt-in.

Stock is held the way a capped discount code is:

- Adding to a cart, or raising a line, is refused (`out_of_stock`) when less is left
  than the cart wants. What other checkouts hold counts, the cart's own hold does not.
- When the shopper asks to pay, the dashboard reserves the cart's lines with an
  optimistic write on each stream before opening the payment. Two checkouts never take
  the last one twice. A refused or failed payment releases the hold; an abandoned one
  lapses after `RESERVATION_SECS` (its expiry is read, never scheduled).
- The paid order consumes the hold (subscription `timada-orders/stock-usage`), and a
  cancelled order puts its quantity back. Both are idempotent per order.

The read models (`catalog_stock`, `storefront_stock`, and the `stock_state` /
`stock_json` / `in_stock` columns of the product rows) carry what is on the shelf,
worked out when the stock or the product changes. They leave holds out, since a hold
lapses without an event: a page may say "in stock" while the last one waits for
another shopper's payment, and the cart then refuses it.

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
answers from their own mailbox (the row links `mailto:` the address) and marks the
message handled (`support:manage`); a message marked too soon is reopened.

The members who may read the messages (the owners, and the roles granting
`support:read`) are told of each arrival by a platform mail with the visitor's first
name and a link to the inbox, nothing else the visitor wrote. A per-tenant
subscription (`timada-dashboard/message-notifications`) follows `Received`, claims the
notice first (`MessageNotification`, one stream per message, as the order
confirmations do) so a cluster mails once, then mails every recipient that identity
holds an address for (a member who signed up with a provider or a passkey alone has
none). A message older than an hour at the time it is seen is history being replayed
and gets no notice.

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

## Newsletter

The addresses that asked for a shop's news are `Subscriber` streams in the tenant's
store (`timada-newsletter`), one per address: the id is a hash of the normalized
address, so the stream also keeps an address on the list once. An address joins in two
steps (double opt-in). `Requested` records where it asked from (`form`, `checkout` or
`account`) and its language; it counts only once its owner follows the link the shop
mails (`Confirmed`). A shopper signed in with a mailed code has proved their address
already: `POST /store/shoppers/newsletter` writes both events at once, no link to
follow. `Unsubscribed` takes them off the list; asking again later writes a new
`Requested`.

The links carry no secret of their own. Each is the subscriber id with an HMAC under
the subscriber's own key: a confirmation link also names the request it confirms by
the time it was written (`{id}.{requested_at}.{mac}`, valid seven days, stale once
another request or an unsubscription follows), an unsubscribe link only the
subscriber (`{id}.{mac}`, valid until they are erased). They open pages of the shop's
storefront (`{storefront}/newsletter/confirm/{token}`,
`{storefront}/newsletter/unsubscribe/{token}`, from the address saved on the API keys
page), which hand the token back through `/store/newsletter/confirm` and
`/store/newsletter/unsubscribe`; the pages ask for a click rather than act on sight,
so a mail scanner opening a link changes nothing. A shop without a storefront address
cannot have a link mailed, and the sign-up answers 503 `newsletter_unavailable`.

A storefront hands a sign-up in with `POST /store/newsletter` (bearer key with
`store:checkout`, counted like the contact form: `newsletter_signups_per_ip`,
`newsletter_signups_per_key`, `newsletter_signups_window_secs` in `[store]`). The
answer is 202 whether the address was mailed, is subscribed already, or was mailed
less than an hour ago (nothing more is mailed then), so it says nothing of who is on
the list. At checkout, a ticked box writes the companion event `NewsletterOptedIn` on
the cart with `CheckoutRequested` (a shopper event a checkout key may append), and
`NewsletterOptInSet` on the order with `Placed`; a per-tenant subscription
(`timada-dashboard/newsletter-opt-ins`) then records the request and mails the link.
Every node with the tenant open runs it, and the node whose `Requested` commits first
is the one that mails. The mail is the shop's `newsletter_confirmation` template, so
the merchant may reword it per language like the other shop mails.

The address is sealed under a key of the subscriber's own, kept in the same vault as
the shoppers' and the messages' keys and followed by
`timada-dashboard/newsletter-keys`. `Forgotten` (`POST /t/{slug}/newsletter/erase`,
`newsletter:manage`, and the deletion of a shopper account with the same address)
destroys it: the read model drops the row and every link mailed to them stops
working. The merchant reads the list on `/t/{slug}/newsletter` (`newsletter:read`,
from `newsletter_subscribers`).

A campaign is a `Campaign` stream (a ULID): the subject, HTML and optional plain text
the merchant writes on `/t/{slug}/newsletter/campaigns` (`newsletter:manage`), as
templates over the shop's variables and `{{ unsubscribe_url }}`, which every campaign
must link to; it may go to the subscribers of one language only. It is a draft
(`Drafted`, `Edited`) until sent (`SendRequested`), and can then no longer be edited.
Nothing in the event store fires later, so every node sweeps the campaigns going out
(`[newsletter] send_check_secs`, from `newsletter_campaigns`) and mails up to
`mails_per_check` of each campaign's confirmed subscribers not mailed yet (from
`newsletter_deliveries`). Each mail is claimed first: a `Delivery` stream per
campaign and subscriber, written with `original_version(0)`, so a subscriber gets a
campaign once however many nodes send it; a mail the server refuses is recorded
(`Failed`) and not tried again. A sweep that finds fewer owed than a batch writes
`Finished`. Every mail carries `List-Unsubscribe` and `List-Unsubscribe-Post:
List-Unsubscribe=One-Click` (RFC 8058) pointing at the storefront's unsubscribe page,
whose POST takes the subscriber off the list; a shop without a storefront address
holds its campaign back until it has one. Marketing never borrows the platform's
server: a campaign goes through the shop's own mail server only, the send page and
the test mail refuse without one (`mail_server_required`), and a sweep that finds
the server gone holds the campaign back, claiming nobody, until it is set up again.

## Automations

A scenario is a `Scenario` stream in the tenant's store (`timada-automations`, a
ULID): a trigger (an order placed, a review submitted, a message arriving, …) and
the steps under it, a condition holding a Yes and a No lane of steps of its own. The
whole drawing is a `Definition`, kept as JSON in the events: `Saved` rewrites the
draft whole, and `Published` freezes it as the next numbered version, which is what
runs. Kinds of trigger and step are strings and a step's settings are kept exactly as
typed, so no enum is frozen in `events.lock` and a kind a newer node wrote survives
an edit on an older one (it shows as a block that cannot be published).

The editor (`/t/{slug}/automations/{id}`, `automations:manage`) keeps the working
copy in the page, not in the store: every button posts the copy and the page comes
back with the change made, until "Save draft" writes it (with the scenario version
the copy was made from, so a copy someone else saved over is refused). Dragging is a
script on top that presses the same buttons; the "+" menu of a slot and Move / Move
here do everything without it. The list is `automations_scenarios`
(`automations:read`). The details, and what runs, are in `docs/automations.md`.

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
proxies), `database` (the store and the read models), `auth` (secret, session
lifetime, OIDC providers), `mail`, `store_api` (omit to not expose it), `cluster`
(omit for a single node),
`monitoring`. `config/dev.local.toml` is the developer's own file and is ignored by
git; `config/e2e.toml` is what the browser suite runs on. Configuration is written for
operators: it names what things do, not the libraries behind them.

## Testing

- `cargo test --workspace` (inside `nix develop`): the crates' domain tests over a
  temporary store, and the dashboard's integration tests, which boot the runtime in
  process: `cluster` (three nodes in one process: a tenant's write lands on every
  node in that tenant only, a node that missed writes catches up, a node away too
  long is given up and copies the store back at its next start, a node at its write
  backlog refuses writes), `store_api` (TCP, keys, TLS),
  `storefront` (the library against a live store), `events_lock`. The review endpoint is
  covered over HTTP by the browser suite.
- `make e2e`: `scripts/e2e.sh` builds the dashboard and the example storefront, resets
  the database, starts the server on `config/e2e.toml` and runs Playwright through the
  compose stack (`make up`: Traefik for HTTPS, dex as an OIDC provider, MailDev as the
  mailbox the tests read codes from). Pass a spec path or a title filter to run part of it.
- `make check` runs tests, clippy with warnings denied, rustfmt and cargo-machete.
