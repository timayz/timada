# Architecture

Timada is a multi-tenant e-commerce back office with a headless storefront library.
A merchant (a *tenant*) signs in to the dashboard, invites a team, manages a catalog
and mints API keys; the merchant's own web app depends on `timada-storefront`,
connects with a key and sells from that catalog in its own markup.

Everything is Rust. The web layer is [topcoat](https://crates.io/crates/topcoat)
(server-rendered pages, `view!` components, cookie sessions); state is event-sourced
with [evento](https://github.com/timayz/evento) over Turso, replicated by its Accord
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
| `crates/pages` | `timada-pages` | The shop's own pages (legal notice, terms, about) in Markdown: drafts, publication, pictures; the storefront picks them by slug |
| `crates/automations` | `timada-automations` | The scenarios a shop's team draws: a trigger, then a tree of steps, kept as a definition the editor works on (see `docs/automations.md`) |
| `crates/invoicing` | `timada-invoicing` | The invoices and credit notes of the orders, numbered under French rules, drawn as PDF (see `docs/invoicing.md`) |
| `crates/storefront` | `timada-storefront` | The library a merchant app depends on (see `docs/storefront.md`) |
| `crates/loadtest` | `timada-loadtest` | Crowds of random shoppers on a storefront over plain HTTP, paying through a stand-in for Stripe: latency, capacity and anomalies (see `docs/loadtest.md`) |
| `crates/core` | `timada-core` | What every context shares and none owns: the paused-store refusal, languages and time zones, the personal-data keys (`keys`), the mailed-code MAC (`codes`), and the one form of an e-mail address, a slug, a currency, the clock, a calendar date, an amount and a `LIKE` pattern (`email`, `slug`, `currency`, `time`, `civil`, `money`, `sql`). Depends on evento only, never on a database driver or the web framework; every context depends on it |
| `dashboard` | `timada-dashboard` | The composition root: runtime, HTTP, auth, store API, CLI |
| `storefront` | `storefront` | The merchant app on the library: a French shop with no brand and no colours, to start your own from, tried on the `seeds/chatounette` snapshot and driven by the browser suite |
| `tests/e2e` | | Playwright suite against the running dashboard |
| `events.lock` | | Every persisted shape, frozen (see *Evolving events*) |

Dependencies point one way. Every context crate depends on evento and
`timada-core`, and a few on another context whose values they share:
`timada-discounts` on the catalog (currencies and amounts), `timada-orders` on the
catalog (it prices an order from the products it loads) and on the discounts,
`timada-reviews` on the orders (a reviewer bought what they review),
`timada-shoppers` on the orders (the shape of an address), `timada-aliexpress` on
the catalog, the import and the orders (what it fills, the orders it forwards) and on
tenancy (the shop's price rule and buyer account); it and `timada-catalog-import` are
the context crates that call out, with reqwest. `timada-invoicing` draws with Typst
behind its `pdf` feature. The storefront library depends on every context a shop
reads (catalog, orders, discounts through them, reviews, shoppers, support,
newsletter, pages, invoicing) and on topcoat; the dashboard depends on every crate;
nothing depends on the dashboard.

## Bounded contexts

A context crate is pure domain code: aggregates, commands, read-model queries. It
knows nothing about HTTP, cookies, files, consensus or who is allowed to do
what; the host decides permissions and hands the crate verified facts.

Each crate follows the same layout, copied from evento's `examples/bank`:
`aggregator.rs` (the `#[evento::aggregate]` enums, names pinned to
`timada-<ctx>/<Enum>` because the aggregate type is persisted), `command/` (the
verbs, a file per aggregate or theme, on a `Command<E>` gateway that holds the
executor and, in a tenant-scoped context, the tenant), `query/` (one file per read
model), `error.rs` (the refusals, each with `is_user_facing` and `code`, the stable
code a page or a storefront words in its own language), `value_object.rs`.

Recurring patterns:

- **Uniqueness without a unique index.** A slug, an e-mail address, a session or API
  token is an *index aggregate* with a deterministic id (`hash_ids([kind, value])`).
  Appending its first event at version 0 is an atomic create-unless-exists, and loading
  it reads the event store directly, so a lookup sees its own writes. A command claims
  the value, then writes its holder; a node may die in between, so a claim never waits
  for a release: a slug or a code whose holder is gone for good (archived, deleted),
  wrote past the claim without taking it, or never got written within
  `timada_core::claim::CLAIM_GRACE_SECS`, is taken over by the next claim. A product's
  and a page's claim record the holder's version it was made at for that. The grace
  compares the claim's stamp, which the cluster sets, with the clock of the node
  asking: the nodes' clocks must agree to well within it (they run NTP). A holder
  write that failed keeps its claim unless the holder has since been written past
  it, since a write the store could not answer for may still land. The entries
  themselves are freed by a release subscription per context (`timada-catalog/releases`,
  `timada-discounts/releases`, `timada-pages/releases`) on what gives a value up, from
  the holder as it is now: only what it holds no more and claimed before the event, so
  a value taken back since stays its own.
  An entry for
  something another stream also records (a user's sign-in identity) counts only while
  that stream lists it: the entry is claimed first and given back if the record
  fails, the record goes first on the way out, and a subscription frees what a crash
  in between left behind (`timada_identity::index_release_subscription`).
- **Every event says who did it** (`*_by` fields). Authorisation lives in the host.
- **Write-side state** is a strict projection with `#[evento::snapshot(none)]`, so
  commands observe the exact version they write against.
- **Read models** are SQL tables in the shared database, fed by one
  `read_model_subscription` per crate. They are eventually consistent; pages that must
  show a write immediately read the aggregate.
- **A saga never fails its pass for a tenant's or an outside service's fault.** A
  subscription's cursor serves every tenant: a handler that returned an error would
  hold it for all of them until that one shop's mail server, or that service, was
  back. A send that fails for such a reason is recorded on the stream
  (`MailDeferred`, `MailFailed`) and the event acknowledged; the node's mailroom
  (`dashboard/src/mailroom`) re-drives what is owed on a timer, a lane per shop, and
  the subscription itself only writes what is owed into the mailroom's table. Only a
  store fault fails a pass. See `docs/mail.md`, "Sending in the background".
  The sagas that issue invoices, answer checkouts and take payments tell the two apart
  with `dashboard/src/saga_fault.rs`: a typed domain error is the event's own and is
  recorded on its stream (`Order::InvoicingFailed`, `Order::CaptureFailed`, a refusal
  of the checkout), then given up on as an evento dead letter (the subscription's
  `.dead_letter()` mode, told by `saga_fault::fault` whose error it is), which
  `/admin/cluster` lists for a week; the merchant or the shopper re-drives it
  (`Order::InvoicingRequested`, "Take the payment again", paying again). Anything
  untyped is a read of the store that failed, and fails the pass. Calls to outside services inside a pass are bounded:
  VIES is asked once for five seconds (the invoice then carries the buyer's number as
  "not checked", VAT charged), Stripe answers reads in five seconds and writes in
  twenty, or the call counts as no answer.
- **A saga whose step is slow answers off its cursor.** The payments subscription
  queues each request to pay with the node's `Checkouts`
  (`dashboard/src/payments/subscription.rs`) and acknowledges it; the lanes answer up
  to 16 carts at once, a cart's requests in order, and a store fault puts a request
  back for a growing while rather than holding the cursor (#467). A request the
  retries cannot answer is refused by the sweep of unanswered requests at its claim's
  end. The same shape as the mailroom: the cursor only records what is to be done.
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
data/timada.db            the store: every tenant's events (the platform's own in the
                          default tenant ""), the snapshots, the subscription cursors
                          and the consensus journal — what the cluster replicates —
                          and the read-model tables (tenancy_tenants, catalog_products,
                          orders_orders, ...), each row carrying its tenant id, derived
                          from the events and rebuilt from them when emptied; `-wal`
                          and `-log` next to it belong with it
data/media/<id>/          one per tenant: the product pictures, the review photos, the
                          pages' pictures and the mail logo, each as two WebP renditions,
                          400/<image id>.webp and 1200/<image id>.webp
data/shopper-keys/<id>/   one per tenant: each shopper account's, newsletter subscriber's and
                          contact message's key, sealed, at <first character>/<key id> (an
                          empty file once destroyed)
<merchant app>/storefront.db  the shop's replica (`STORE_DB`): subscription cursor,
                          snapshots, product and category rows
```

The store is a Turso database (SQLite's format in a pure-Rust, in-process engine)
opened for several writers at once (`dashboard/src/db.rs`, `open_store`): the node
applies events on the store's own connection, the consensus journal writes on its own,
and a read model in the file is written by its subscription through
`Turso::transaction`. Every write is a `BEGIN CONCURRENT` transaction that runs
alongside the others and is run again should two of them change the same rows, so an
acknowledgement from one of the twenty-odd subscriptions never waits on the journal's
fsync. The pages read the same file through `timada_core::db::Db`, a thin layer over
the store's connections (`fetch_*`, `execute`, `write` for a handler's transaction).

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

1. **Cluster** (`cluster.rs`). One Accord node per process over the store, opened
   from the `[cluster]` section by evento's host (`evento::cluster::Cluster::builder`,
   with `ClusterConfig::settings()` as its `Settings`): the node, its inbox loop and
   its recovery sweep, over TCP with the `[cluster]` peers (a majority must agree on
   every write) or an in-memory transport alone. Reads, snapshots and
   subscriptions are served from this node's own file. With several nodes a
   single-aggregate read through the store waits for the writes a quorum already
   committed (`linearizable_reads`, a consensus round per read), so a page that reads
   what it just wrote sees it whichever node answers. `cluster::open` also hands out
   the same store read from the node's file as it stands (`LocalReads`, evento's
   `ClusterExecutor::local_reads`; its writes still go through the cluster, and a node
   reads its own writes either way) for the reads that need no such round: every
   subscription, and the mailroom and the checkout lanes they feed, whose handlers
   read what the node applied before handing them the event (a write on a stale read
   is refused for its version, and the write it missed is applied here before the
   refusal, so the retry sees it); every timer (the payments sweeps, the hold
   reminders, the idle carts, the review requests, the newsletter sends, the
   automation runs, the AliExpress sync), which acts off read-model rows the node
   derived from that file and writes only at the version it loaded, or a claim of its
   own, so the same refusal holds (the payments and AliExpress services hold no store
   of their own: a shop and its company are read from the store their caller reads,
   the node's file for these and through the cluster for a page); the store API's
   connections; the shop's offers,
   codes and ways to ship on `GET /store/carts/{cart_id}` (the cart itself is the
   shopper's own stream, read through the cluster, and so is a line the node's catalog
   read model cannot sell, which the cart took after reading the store: a product put
   on sale or stock put back a moment ago) and the wait for the answer to a
   request to pay; and what every request asks first, the session and its user, the
   tenant, a storefront's key, which is asked of the cluster only before it turns the
   request away (`LocalReads::first`: a sign-in, a tenant, an invitation accepted, a
   key issued or a suspension lifted a moment ago on another node). A browser that may
   have written a moment ago reads those through the cluster (`reads.rs`: the answer
   to any post, and any redirect, which is how the sign-in and connection callbacks
   end, sets the `timada_wrote` cookie for ten seconds), so the page a form sends it
   back to shows the change. Every finished write is kept on the other nodes until
   the node that missed it has it, so a node that stays down makes the others keep everything, up
   to `cluster.write_backlog` writes, past which this node refuses writes: a page then
   shows that changes are temporarily not accepted (the `writes_paused` code of
   `timada_core::write_refusal`, which the `is_user_facing`/`code` of every context a
   page talks to recognise), the store HTTP endpoints answer 503 with that code, and
   the store API
   carries it as a refusal a storefront shows the same way. A node not heard from for
   `cluster.absent_after_secs` (a day by default: the backlog holds days of writes,
   and copying a large store back costs far more than catching up) is therefore
   given up: the others
   tidy up without it, and when it returns they tell it it fell behind. It then
   refuses writes, logs the procedure and marks the store for a copy (the `rejoin`
   file next to the store, written by evento's host); the next `serve` empties the
   store (events, snapshots, cursors; the journal stays), drops and recreates the
   read-model tables (the host's `before_copy` hook, `db::reset_store_read_models`:
   a drop is one schema change where deleting the rows would write every one of
   them to the database's log once more) and opens the node by
   joining, which installs a peer's store page by page (4 MiB at a time) before
   anything is served; then the read-model subscriptions catch up from it, all at
   once (each writes tables of its own), before the node listens. The pictures and
   keys this node keeps are not in the store. Before the read models catch up, it
   copies every key from a peer, a page of a thousand at a time (`GET
   /cluster/keys`, then `GET /cluster/keys/<tenant>?after=<key id>`), as the
   read models open each shopper's data with its key and would otherwise ask the
   peers for each in turn; a peer that fails part of the way leaves the rest to
   the next, and what none hands over is asked for key by key. The pictures'
   subscriptions walk their events again from the start and
   fetch from a peer only what the disk lacks, so a node rebuilt on a new machine
   should get `data/media` copied from a peer (`rsync`) before its first `serve`;
   a picture a node has not got yet is answered 404 with `Cache-Control:
   no-store`, so no cache in front keeps the miss. The
   `rebuild <read-model>…` command (server stopped; `rebuild --list` names them)
   does the same for one read model after a change to its tables or handlers:
   it drops and recreates its tables, forgets the cursors of the subscriptions
   that fill it, and leaves a `catch-up` mark so the next `serve` catches up
   before it listens; on a cluster, one node at a time, the others serving
   meanwhile. The
   `GET /ready` (`readiness.rs`) tells the balancer whether to send traffic here:
   200 while the node takes writes and no read model trails the store by more
   than `monitoring.ready_lag_secs`, 503 otherwise, the body saying how far each
   read model trails (its subscription's recorded lag, raised to the age of the
   store's newest event when a worker stopped acknowledging). A node back from a
   night away, or one that is catching up after a copy or a `rebuild`, is left
   out until it reads low. `monitoring.metrics_listen` adds an internal port
   with `/metrics` (`metrics.rs`: Prometheus text with the same figures, the
   workers by state, the writes kept for absent nodes, the store connections by
   tenant and evento's counters, every series labelled by node) and `/ready`. The
   `rejoin` command sets the same mark by hand, for a lost store file (delete the
   file first: an empty journal comes back under the node's own id through evento's
   amnesia fence). A node back before anything was tidied up past it is simply waited
   for again. A journal found to be an older copy of itself is marked the same way,
   journal included. `/admin/cluster` shows this node (`Cluster::health`): the writes
   it keeps, how each other node stands, whether a copy is needed; `Cluster::watch`
   warns in the log at four fifths of the backlog and whenever a node is given up.
2. **Subscriptions** (`serve.rs`, `subscriptions()`). Every subscription the node runs
   is started once on the store and follows every tenant's streams: the platform's
   read models (users, the tenant catalog, the members' history), the tenants' read
   models, this node's copies of the pictures and the
   personal-data keys, the answers the dashboard writes back (payments, invoices),
   the mails it sends (order mails, message notices, newsletter opt-ins),
   the automation runs it starts.
   A handler takes the tenant from `event.tenant` and builds the gateway it writes
   back with from it. Adding a tenant-scoped context means adding one entry there.
3. **Store API** (`store_api.rs`). An evento-remote listener. Each connection opens
   with an API key token; the resolver loads the key by the token's hash (from the
   node's file, asked of the cluster before a refusal) and serves the connection,
   reading from the node's file, in the key's tenant (`Session::tenant`): every request acts in that
   tenant whatever it names, the events it reads come back as from a store of its own,
   and the subscriber calls are refused (a storefront keeps its cursors locally). A
   key without the `store:write` scope gets the store wrapped by `read_only_store.rs`:
   appends refused, snapshot saves dropped, and reads confined to what a storefront
   loads (`store:read`: products, categories, stock, reviews, pages; `store:checkout`
   adds the payment method, shipping, business buyers, discounts, offers, review
   links). Never the carts, the orders or the invoices: the cart and order streams
   are every shopper's cart and every placed order with the addresses they carry, and
   an invoice series (`timada-invoicing/Book`) is every invoice the shop issued with
   its buyer's name, address and e-mail, so a storefront is answered about one at a
   time, by the id it holds, through the dashboard's HTTP side (`store_carts.rs`
   behind `/store/carts/*` and `/store/orders/{order_id}`, authenticated with the
   same key, which must allow `store:checkout`): its cart priced, the items it adds,
   the code it enters, its request to pay and the answer, the order its cart became,
   an order it remembers, its shopper cancelling it while its goods are not bought from
   the supplier yet (`/store/orders/{order_id}/cancel`), and that order's invoice and
   credit notes (`/store/orders/{order_id}/documents`, which the storefront draws as PDF
   itself) and where its parcels are (`/store/orders/{order_id}/parcels`).
   An id the tenant has no cart or order under, another tenant's included, is 404.
   A cart starts with its first item, checked first and written in one go, so an item
   refused leaves nothing; each key, and each visitor at a shop (`X-Shopper-IP`), starts
   only so many within `[store]`'s window (`carts_per_key`, `carts_per_ip`; 429
   `too_many_carts`). A cart nobody touched for `[carts] idle_days`, with no payment
   open, none on its way back and no request to pay unanswered, is dropped
   (`Cart::Expired`, `idle_carts.rs`: each node on a timer, its own part of the carts
   among the nodes up): every command and reader then finds no such cart, a storefront
   starts another, and its row leaves `orders_carts`; its events stay in the store.
   What a shopper does is therefore written by the dashboard in the key's tenant, and
   a storefront's connection never appends: a read naming any other stream of the
   tenant, a shopper's sign-in challenge say, fails, and so does every write. Scopes are strings recorded with
   `ApiKey::Created` and replaced whole by `ApiKey::Scoped`. Revoking the key or
   changing its scopes ends its connections; suspending the tenant ends every
   connection of the tenant, and reconnects are refused. Each node holds its own
   connections and checks a key only when one opens, so this happens on every
   node: the page that took the request ends its node's at once, and every node
   runs `store_api::closing_subscription` for the others. It starts before the
   store API takes a connection, reads from the head of the store with its cursor
   in memory, and claims nothing. An event written before it started (less a
   margin for the clocks) is already in every connection's key, so it is skipped:
   right after the whole cluster restarts, the store may not know its head yet,
   and the subscription then reads from the beginning.
4. **Payments** (`payments/`). Each tenant connects its own Stripe account on
   `/t/{slug}/payments`; the secret key and the webhook signing secret are recorded
   in the tenant's platform events sealed with AES-256-GCM (`seal.rs`) under the
   operator's `[secrets] key` (no section, no payments), bound to the tenant: every
   sealed secret names its owner (the tenant, the company, the key's tenant and id,
   the webhook step) as authenticated data, so bytes copied into another owner's
   events open nothing. A storefront asks to pay through
   `POST /store/carts/{cart_id}/checkout`, which writes `Cart::CheckoutRequested`
   and waits for the answer; the subscription `timada-dashboard/payments`, on the
   node that claims the request first (`Cart::CheckoutClaimed`, written at the version
   read; the other nodes leave it alone, and the claimant goes on after an error or a
   restart), holds the goods and the code, prices the cart from the store, opens a
   PaymentIntent with the tenant's key, held only (manual capture; the request id is
   the idempotency key, so a retry opens the same payment, and Stripe still opening it
   (409) is asked again) and records it (`Cart::PaymentRequested`, with the supplier that
   buys some of its goods once the order is placed, if any) or the refusal, which alone
   gives the holds back. The shopper
   confirms with Stripe's card form, which holds the money; Stripe calls
   `POST /stripe/webhooks/{tenant_id}`, any node, which checks the signature, reads the
   payment back from Stripe and acts on its state (`payments/webhook.rs`): held, the
   order is placed (`place_order`, status `Authorized`); taken, it is recorded; dropped
   (the week a card holds money ran out), the order is cancelled, unless the supplier may
   have it (its fence written, no cancellation asked): the hold is then recorded gone
   and the order left open for the merchant. A second payment of a
   cart, and one confirmed after the checkout's holds lapsed and went to someone else,
   are given back, nothing taken: the cart records that the payment goes back and why
   (`Cart::PaymentReturnRequested`) before Stripe is asked, and that it went
   (`Cart::PaymentReturned`) after, only what Stripe has not refunded yet being refunded
   (a refusal for good settles it too, `Cart::PaymentReturnRefused`, shown on the order
   for the merchant to settle in Stripe), and money Stripe took for an order cancelled
   meanwhile goes back through the order (`Order::RefundRequested`, then Stripe,
   `Refunded`). The money is taken
   (`payments/settle.rs`) once the order asks for it (`Order::CaptureRequested`): at
   once by the subscription `timada-dashboard/order-payments` when the shop holds every
   good, by the same once AliExpress took the order, or by the merchant; Stripe's answer is
   `Order::Paid` (the invoice follows) or `Order::CaptureFailed`. Anything irreversible
   (taking the money, giving it back, ordering from the supplier) is first written on the
   order at the version read, so two of them never cross: a pending capture holds off a
   cancellation and the other way round. Cancelling writes `Order::CancellationRequested`,
   then gives the hold back (`PaymentReleased` with `Cancelled`), or refunds the money
   taken first (`Order::RefundRequested`, the idempotency key, then Stripe, `Refunded`,
   `Cancelled`); `charge.refunded` reconciles refunds Stripe holds that the order does not
   know. No subscription handler fails on a Stripe answer, but for the checkout's 409
   (Stripe still opening the payment under the same key), which it asks again in a
   moment: what Stripe could not be asked at once (a capture, a cancellation, a refund, a
   payment to give back) is settled by the sweep `payments/pending.rs` a minute later,
   and a request to pay nobody answered is refused by `payments/unanswered.rs`. All of it
   needs the tenant's keys, so stopping payments (`Tenant::PaymentsStopped`) stops new
   checkouts at once but keeps the keys, sealed, until the merchant forgets them
   (`Tenant::PaymentsForgotten`): never before the stop is eight days old (past a card's
   longest hold) nor while the orders read model shows anything held or owed, and no
   sweep forgets them on a read model's word. Saving keys again takes payments again. Setting it up and testing it locally: `docs/payments.md`.
5. **Shop mail** (`shop_mail.rs`). A merchant sets up SMTP servers of its own on
   `/t/{slug}/mail` (up to ten) and picks one for each kind of mail
   (`MailServersPicked`). The shop's mails to its shoppers (order mails, review
   requests, sign-in codes) go out through the server picked for them, and
   through the platform's `[mail]` server under the shop's name when none is
   (`ShopMail::sender_of`). Saving a server sends a test mail first. The passwords
   are recorded in the tenant's platform events (`MailServerAdded`/`Changed`)
   sealed under `[secrets] key`; secrets never enter the tenant store, which
   replicates to storefronts. Hosts on this machine or a private network are refused
   unless `mail.allow_private_hosts` is set, and the transport dials the addresses
   that check resolved, in turn, rather than the name (TLS still verifies the name),
   looking the name up again every few minutes, so a name that later answers a
   private address (DNS rebinding) gets nowhere. Account codes and invitations always go
   through the platform's server; newsletter campaigns, automated mails and their
   test mails never do, they go through the server picked for marketing only
   (`ShopMail::marketing_sender_of`). Each shop mail may be worded per language: the
   shopper's language comes from the storefront's `Accept-Language` and is kept on
   the order (`LanguageGiven`/`LanguageSet`). Details: `docs/mail.md`.
6. **Authorisation** (`authz.rs`). Casbin RBAC with the tenant as domain and its custom
   roles as roles. Policies are not loaded at startup: every tenant-scoped request loads
   the tenant's strict state, whose version says which events it reflects, and the
   enforcer re-syncs a tenant's rules when a newer state comes along. Owners bypass.
   `manage` on a resource implies `read`. Shaping the roles and moving members between
   them is for owners alone, whatever a role grants (a member who could edit roles
   could widen their own); `members:manage` invites and removes members, with a role
   no wider than its holder's.
7. **Auth glue** (`auth/`). The web side of the identity context: OIDC discovery and
   token exchange, WebAuthn ceremonies, mailed one-time codes, the private cookies that
   carry a sign-in attempt between steps, and the session cookie. A password sign-in
   is counted before the password is looked at, per browser address and per account
   (`[auth] login_attempts_per_ip`, `login_attempts_per_email`, `login_window_secs`;
   `too_many_logins` past them), so a password cannot be guessed by trying. Like every
   limit here (`rate_limit.rs`), it holds across the cluster: each node keeps the passes
   it let through and, before letting one more through, asks the other nodes for theirs
   (`POST /cluster/limits` on their `cluster.web_peers` address, under `cluster.secret`,
   the keys hashed). A node that does not answer within two seconds counts for nothing,
   and two nodes deciding at the same moment may each let the last pass through, so the
   limit holds to within a moment's requests.
8. **Pages** (`app/`). The topcoat router; the module tree is the URL tree.
   `/auth/*` (sign in, the account page, passkeys, e-mail, password, sessions),
   `/tenants/new` and `/t/{slug}` (members, roles, API keys, products, categories,
   orders, open carts, customers, discounts, checkout offers, shipping, reviews,
   messages, newsletter, automations, pages, payments, invoicing and the invoices,
   mail, AliExpress, the shop's name and time zone), `/companies` and
   `/companies/{id}` (the legal entities the shops sell as, their AliExpress buyer
   account), `/aliexpress` (the platform app's callback), `/stripe/webhooks/{tenant_id}`,
   `/store/reviews` and `/store/reviews/revise` (a storefront handing in a review, see
   *Reviews*), `/store/shoppers/*` (a storefront signing its shoppers in, see *Shopper
   accounts*), `/store/contact` (a storefront handing in a message from its contact
   form, see *Support messages*), `/store/newsletter/*` (sign-ups and their links, see
   *Newsletter*), `/store/automations/opt-out` (a customer declining a shop's
   automated mails), `/cluster/media/*`, `/cluster/keys/*` and `/cluster/limits` (the
   nodes copying pictures and keys to one another and asking what the others let
   through the limits, under `cluster.secret`), `/invitations/{token}`,
   `/admin/*` (users, tenants, companies, this node's cluster view). `ui/` holds the
   shared furniture: the chrome, the page helpers, the theme, the icons.

Shutdown stops the HTTP server, then the store API, the sweeps, the subscriptions,
the node and finally the pools.

## One write, end to end

A member renames a product on `/t/{slug}/products/{id}`:

1. The page handler loads the session, the tenant state and the enforcer's answer for
   `products:manage`.
2. `timada_catalog::Command::new(store, tenant_id)` is the catalog gateway in the
   tenant's streams.
3. `rename_product` loads the product's write-side state in that tenant, checks the
   invariants, claims the slug of the new name at the product's version and commits
   `Renamed { name, slug, renamed_by }` at that version. The old slug is free to the
   next claim at once; the subscription `timada-catalog/releases` frees its entry.
4. The commit goes through the Accord node and lands in `data/timada.db`, in the
   tenant's stream of that product.
5. The `catalog` subscription writes the new name and slug into `catalog_products`, under the
   event's tenant; the next page load shows it. The
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

An option can be *hidden* from shoppers at one of its values (`OptionHidden`), which
can change at any time, like labels: an imported product may vary by where it ships
from, and the merchant picks that instead of the shopper. Only the variants taking the
value are on sale (`ProductState::sold_variants`). The others stay as they are, so an
import still finds them, but a cart refuses them and every shopper-facing number leaves
them out: the price range, the stock summary, the storefront's rows and filters. A
line's label leaves the option out too, and a shop's copy of the product does not know
it exists. The value must leave something on sale; with every option hidden, that is
one variant, which the shop sells with nothing to pick.

Codes are unique across a tenant through the same index-aggregate pattern as slugs
(`timada-catalog/ProductSku`), compared without regard to case. A variant that goes,
one whose code changes, and an archived product, give their codes back.

A product leaves the catalog one of two ways. `Archived` is final and keeps it on
record: no more edits, its slug and codes free again, its row still listed as
archived. `Deleted` is for a product nobody ever ordered: it is archived and, on
top, every read model drops its rows and the host deletes its pictures' files, as if
it had never been. The catalog cannot see the orders, so the dashboard checks the
orders read model (`orders_order_products`, one row per product each order holds)
before it deletes; a product once ordered can only be archived.

## Stock

A shop may count what it holds of a product without variants, or of each variant.
Each counted thing has streams of its own (`timada-catalog/StockItem`, deterministic
id from the product, the variant and a generation number), so a product's stream
stays short however often it sells. A thing without a stream, or untracked, sells
without limit: counting is opt-in.

A thing's stock is written in generations of at most `STOCK_CHUNK` (1000) events,
because every checkout writes to it and a stream has 65 535 versions: a thing that
sold well would otherwise stop selling after some thirty thousand checkouts.
`timada-catalog/StockPointer` (one stream per thing) names the current generation.
The writer that finds a generation full opens the next one with `Opened`, carrying
what is on hand, the holds not lapsed and the last `RECENT_ORDERS` (256) orders,
then rolls the pointer and tries its write again. Nothing is ever appended to a full
generation (every writer checks the version it read against the cap), so every
writer that opens the next one carries the same state and the first to write it
wins; the pointer's own stream grows by one event per roll. What a paid order took
is also written to a stream of the order's own (`timada-catalog/StockTake`), once
the stock streams are: that is what makes taking, and giving back on cancellation,
happen once however often the order is delivered, long after the generations forgot
it. The read models key stock by product and variant, never by stream id, so a roll
changes nothing they show. Capped discount codes keep their uses the same way
(`timada-discounts/DiscountUsage` in generations of `USAGE_CHUNK` events, with a
`DiscountUsagePointer`; `Opened` carries the live reservations, the count of spent
uses and the last `RECENT_USES` orders).

Stock is held the way a capped discount code is:

- Adding to a cart, or raising a line, is refused (`out_of_stock`) when less is left
  than the cart wants. What other checkouts hold counts, the cart's own hold does not.
- When the shopper asks to pay, the dashboard reserves the cart's lines with an
  optimistic write on each stream before opening the payment. Two checkouts never take
  the last one twice. A refused or failed payment releases the hold; an abandoned one
  lapses after `RESERVATION_SECS` (its expiry is read, never scheduled).
- The order consumes the hold once placed, its money held (subscription
  `timada-orders/stock-usage`), and a cancelled order puts its quantity back. Both are
  idempotent per order.

The read models (`catalog_stock`, `storefront_stock`, and the `stock_state` /
`stock_json` / `in_stock` columns of the product rows) carry what is on the shelf,
worked out when the stock or the product changes. They leave holds out, since a hold
lapses without an event: a page may say "in stock" while the last one waits for
another shopper's payment, and the cart then refuses it.

## Product pictures

The event store records what a picture is, never its bytes. The upload itself is not
kept either: the node that takes it decodes it (JPEG, PNG, WebP or GIF, at most 8 MiB
and 25 million pixels, turned the way its EXIF orientation says) and renders two WebP
files from it, a listing rendition of at most 400 px on its longest side and a page
rendition of at most 1200 px, never enlarging (`timada_core::Picture`,
`dashboard/src/media.rs`). The event records the upload's dimensions and, for each
rendition, its dimensions, its size and the SHA-256 of its bytes. So a shop's pictures
weigh a few tens of kilobytes each whatever the merchant uploaded, a listing sends a
browser the small one, and a product page sends the large one with a `srcset` a phone
takes the small one from (`timada_storefront::product_image`, `large: true`). The files
land in `media.dir/<tenant>/400/<image id>.webp` and `.../1200/<image id>.webp` and
are served at `/media/...` by a directory route with `Cache-Control: immutable`, since
a new picture is a new id and an address never changes what it holds. There is no
authentication on it: an id is an unguessable ULID, and a shopper's browser has no
session on this node. A page's body names a picture by `<image id>.webp`;
`timada_pages::render_markdown` serves the page rendition for it.

An upload renders (off the async threads, two at a time), stores the files, and then
commits, deleting them again if the commit fails; a removal commits and then deletes.
So a failure leaves files nobody references rather than a product pointing at nothing.
`media.public_url` is the address a shopper loads a picture from, shown on each
tenant's API keys page for its storefront to be configured with.

With several nodes, every node keeps every picture, so a node going down takes none
with it and `media.public_url` can point at any of them or at a balancer in front.
The node taking an upload stores the renditions, copies each to the other nodes
(`PUT /cluster/media/<tenant>/<400|1200>/<file>` on their `cluster.web_peers` address,
with `cluster.secret` as a bearer token; a copy is taken only if it is WebP and no
heavier than a rendition gets) and commits only once a majority of the cluster holds
both, the same majority the commit itself needs; short of that the upload is refused
and the copies are left unreferenced. Each node of that majority syncs the files to its
disk, names and all, before it answers, so a power cut across the cluster keeps the
picture with the event; a copy a node fetches has its bytes synced before its name, so
no node serves an empty file after a power cut.
Copies to the slower nodes carry on after the
commit. Each node also runs the `media` read model (`timada-dashboard/media` for the
catalog, the pages and the reviews, `timada-dashboard/mail-logos` for the tenants):
`media_files`, one row per rendition file the store knows with its size, digest and
what references it, the same on every node; and `media_wanted`, the node's own list
of files to fetch, which a per-node fetcher (`dashboard/src/media/fetcher.rs`) works
off the subscription's cursor, several files at once, keeping a copy only if its size
and digest are the ones recorded. A file every peer lacks is dropped with a warning
(gone for good); a file some peer could not be asked for waits, longer each time, and
is given up on after twelve hours, which `/admin/cluster` shows for a week. So a
picture missing everywhere never stalls the subscription (#468, from #464). The
subscription deletes the files of the pictures removed, by what referenced them, with
no stream read. A miss on `/media` for a file the store knows fetches it from a peer
before answering, so a node that has not got a picture yet serves it all the same; a
miss for a file the store does not know is a 404 at once, no peer asked.

A tenant's pictures may take `media.tenant_quota_bytes` (5 GiB by default; 0 is no
quota) unless the platform set a quota apart for it on its admin page
(`MediaQuotaSet` on the tenant, in MiB on the form); what it uses is the sum of its
rows in `media_files` (`dashboard/src/media/quota.rs`), shown on its API keys page.
An upload, a review's photos or an import's picture that would go past it is refused
(`storage_full`, `review_storage_full`; an import leaves the picture out). The check
reads the read model as it stands, so two uploads landing together may overshoot by a
picture. Once a day (`media.reconcile_check_secs`) each node walks `media.dir`
(`dashboard/src/media/reconcile.rs`): a file the store does not know is deleted once a
day old (an upload whose commit failed, a node that died between storing and
recording), a file held at another size than recorded is deleted and wanted again, and
a file the store knows and the disk lacks is wanted; the run is logged.

In production put a cache in front of `media.public_url` (a CDN, or the proxy's own
cache): a picture is served with `Cache-Control: public, max-age=31536000, immutable`,
so the cache may keep it for good, and the nodes then serve each file about once. Have
the cache serve what it holds when a node is down (`stale-if-error`, or the proxy's
equivalent), so a storefront kept up by its own copy of the catalog keeps its pictures
too (#454 covers the HTML; the pictures are the dashboard's). A node rebuilt on a new
machine should still get `data/media` copied from a peer (`rsync`) before its first
`serve`; the fetcher and the misses cover what the copy lacks.

## Reviews

Only a buyer reviews: a review is written from a paid order (its money taken, not only
held), for a product on it, and
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
not there, over the store API: a storefront's key appends nothing there
(`read_only_store.rs`). The
storefront vouches that the shopper holds the order: an order id is not guessable, and a
storefront offers the form only on an order its own shopper placed. The media
subscription keeps review photos on every node like product pictures, and deletes them
when a review is rejected or a photo is taken off.

Some days after an order is fulfilled (7 unless the merchant picks 0 to 60, or turns
it off, on the Reviews page: the `ReviewRequests` stream), its buyer is mailed under
the shop's name with a link to `{storefront}/reviews/{token}`. Nothing in the event
store fires later, so every node runs a timer (`[reviews] request_check_secs`) that
walks the requests the reviews read model says are due and not mailed yet
(`reviews_requests.due_at`, indexed where set, 200 to a statement from where the last
one stopped) and mails those due within the last 3 days: a node that was down catches
up, and a shop that turns the mails on does not mail its history. The read model
follows the orders as they leave (`Fulfilled`) and the shop's policy, so a change of
delay moves the orders that can still be asked about, and a request mailed leaves the
list; the policy as it stands still decides, the read model only says where to look.
The first node to write an order's `ReviewRequest` stream (`original_version(0)`)
sends. Nothing is claimed while the shop has no storefront address, or once every
product of the order is reviewed; such an order stays listed until its 3 days are
over, and the walk goes on past it. The token is 32 random bytes
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
index; `ShopperChallenge` keeps only a MAC of the code under a key derived from
`[secrets] key` (`timada_core::CodeVerifier`; the store never holds it, so a storefront's key
reading the stream cannot try the million codes offline) and counts the wrong guesses (five
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
`data_keys.dir/<tenant>/<first character of the key id>/<key id>`, each sealed with
`[secrets] key` (no section, no shopper accounts and no contact messages): a key id is
random, so a tenant's keys spread evenly over 64 directories, some sixteen thousand
files in each at a million accounts. Like a picture, a key is on a majority of the
cluster before `Registered` commits (`PUT /cluster/keys/<tenant>/<key id>`, with
`cluster.secret`), and each node of that majority syncs it to its disk, name and all,
before it answers (`dashboard/src/disk.rs`), as the store syncs each write: a power cut
across the cluster keeps the key with the event.
Nothing serves these files to the outside: a node lacking a key asks
all its peers at once on the same route, with a `GET` carrying the secret, and takes a
peer that has not answered within two seconds for out of reach. Each node follows the
tenants' `Shopper` streams (`timada-dashboard/shopper-keys`) to fetch the keys of new
accounts and destroy those of deleted ones. Destroying puts an empty file, the
tombstone, in the key's place in one step (a rename), and a key only ever takes a name
no file holds yet (a hard link from a temporary file), so a copy or a fetch still under
way cannot bring a key back. A tombstone's name is synced too, or a power cut would
bring the key back; and every key file's bytes reach the disk before its name, a fetched
one's included, since a power cut would otherwise leave an empty file, which reads as a
tombstone. Tombstones are kept for good: an address whose account was
deleted asks for its old key when it signs in again, and a peer that has not caught up
with the deletion would otherwise hand it back. The read model opens the address for
the Customers list with the key, and fails its pass while a key on its way from a peer
has not arrived, to be started again from that event. A backup of this directory
brings deleted accounts back when restored, so keep it short-lived or skip it; losing
it on every node loses every account's data.

A storefront signs its shoppers in over HTTP, not through the store API, under
`/store/shoppers/*` with its key as a bearer token (it must allow `store:checkout`):
`code` mails the code from the shop's name, `session` checks it and answers the token,
`sign-out`, `sign-out-everywhere`, `delete`, `addresses` and `addresses/remove` act for
the shopper whose token is in `X-Shopper-Token`. A storefront's key appends nothing
over the store API (`read_only_store.rs`), so a key cannot sign anyone in by appending
the shopper events. A storefront resolves a token with
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
subscription (`timada-dashboard/message-notifications`) follows `Received` into the
node's mailroom (see `docs/mail.md`, "Sending in the background"), which claims the
notice first (`MessageNotification`, one stream per message, as the order mails do
with `OrderMail`, one stream per order and mail) so a cluster mails once, then mails
every recipient that identity holds an address for (a member who signed up with a
provider or a passkey alone has none), and records how it went on the notice's
stream. A message older than a day at the time it is seen is history being replayed
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
the cart with `CheckoutRequested` (written by the dashboard for the storefront's
`POST /store/carts/{cart_id}/checkout`), and
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
Nothing in the event store fires later, so every node looks at the campaigns going
out every `[newsletter] send_check_secs` (from `newsletter_campaigns`) and mails its
own part of their confirmed subscribers not mailed yet (from `newsletter_deliveries`).
A subscriber falls to one of the nodes up by the `shard` of their id
(`timada_core::Share`, the rule the AliExpress reads follow), so the nodes send side
by side rather than race for the same mails, and the part of a node no longer heard
from is shared out among the others. Each look starts a lane per shop, up to 16 at
once on a node, the shops served longest ago first: it mails one subscriber after
the other through the shop's server until shortly before the next look or until
`mails_per_check` of a campaign went out, so a fast server sends many and a slow one
holds up its own shop only. A node walks each campaign's list by subscriber id from
where it stopped (kept in memory), not through everyone mailed already, and once a
whole pass over its own part found nobody to mail while someone is still owed, it
walks everyone's: a node that is heard from but does not send (copying its store,
catching up, its writes refused) holds its part back for a pass or two only. Each mail is claimed first: a `Delivery` stream per
campaign and subscriber, written with `original_version(0)` and no read first, so a
subscriber gets a campaign once even while two nodes disagree on who is up; a mail
the server refuses is recorded (`Failed`) and not tried again, and a failure the
server is to blame for, rather than the address, ends the shop's lane until the next
look. A subscriber the read model still owes but who left the list (or was erased)
since is recorded as `Skipped`, so the campaign stops owing them; one whose
personal-data key is not on this node yet is left for the walk's next time through
and holds the campaign open. A node whose walk reached the end of its part writes
`Finished` once the read model owes nobody, whichever node's part. The pages read
the counts from the rows themselves, kept by the read model as the deliveries and
the subscribers change: delivered, failed and skipped on `newsletter_campaigns` and
`newsletter_editions`, the confirmed subscribers by language and time zone in
`newsletter_audience`. Every mail carries
`List-Unsubscribe` and `List-Unsubscribe-Post:
List-Unsubscribe=One-Click` (RFC 8058) pointing at the storefront's unsubscribe page,
whose POST takes the subscriber off the list; a shop without a storefront address
holds its campaign back until it has one. Marketing never borrows the platform's
server: a campaign goes through the mail server the shop picked for its marketing
only, the send page and the test mail refuse without one (`mail_server_required`),
and a sweep that finds none picked holds the campaign back, claiming nobody, until
one is. The sender is looked up at each look, so a server switched mid-campaign sends
the next mails.

A campaign may go out on a schedule instead of at once (`ScheduleSet`: once, daily,
weekly on some days or monthly on a day, between two dates, within a window of the
day, at a pace). Started (`ScheduleActivated`), each cycle of the cadence opens an
`Edition` stream (its id derives from the campaign and the cycle, so a cycle opens
once however many nodes look) holding the copy as it stood, and the sweep spreads
the edition's mails over the window's slots (`crates/newsletter/src/schedule.rs`:
at each slot passed, the share mailed so far grows). The share comes from the read
model's counts (`newsletter_edition_zones`, the edition's mails by time zone, and
`newsletter_audience`): each node mails its part of what a slot owes, and at a
zone's last slot whoever is still owed there. A slot is a time of the local
day: each subscriber's browser said where they live when they asked
(`TimeZoneSet`, an IANA name, kept in clear), so 9:00 reaches Paris six hours
before New York. A one-off edition still owed mails when its window closes rolls
into the next cycle; a recurring one closes. The copy may name
`{{ new_products }}`, the products published since the last edition, so a
recurring campaign writes itself. Stopping (`ScheduleStopped`) opens no further
edition.

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
(`automations:read`).

Publishing freezes the draft as a numbered version, kept as published in a read
model for the runs pinned to it. A trigger event starts a `Run` of every published
scenario listening to it (`timada-automations/triggers`; the run's id derives from
the scenario and the event, so it starts once), and the runs due move on a timer
(`[automations] check_secs`, `runs_per_check`), each move written against the run's
version so one node acts. The nodes share both out: each starts and moves the runs
that fall to it among the nodes up, and takes over any run another node left for
two minutes. The details are in `docs/automations.md`.

## Shop pages

A page is a `Page` stream in the tenant's store (`timada-pages`, a ULID): a title, a
slug unique in the shop (a `PageSlug` index aggregate) and a body in Markdown, kept as
written. The storefront decides which pages it has and where (its own routes, such
as `/mentions-legales`, and its own footer), and reads each one's text by its slug; it
starts as a draft, and only once `Published` does the storefront show it. It carries the title and description
search engines show, and pictures uploaded beside it, kept like a product's (see
*Product pictures*: on a majority of nodes before `ImageAdded`, fetched by the others
through `timada-dashboard/media`, deleted with `ImageRemoved` or `Deleted`). The body
names a picture by its file name (`![The workshop](01J….webp)`).

`timada_pages::render_markdown` turns the body into HTML each time it is shown:
CommonMark with tables, raw HTML shown as text, only the page's own pictures (served
from the shop's media address), and the result cleaned by an allow-list sanitiser.
The back office (`/t/{slug}/pages`, `pages:read` / `pages:manage`) writes a page beside
that rendering in a sandboxed frame; the list is the `pages` read model.

## The storefront side

`timada-storefront` runs inside the merchant's app. Its executor is a `Sidecar`:
events go through the remote client to the store API, while the subscription cursor,
the projection snapshots and the product rows live in the app's own Turso file. One
file per replica, so replicas never fence each other out of the subscription and
nothing accumulates on the tenant's store. Carts and orders never come through that
executor: the library asks the dashboard's `/store/carts/*` and `/store/orders/*`
endpoints for the one its page holds the id of (`StoreConfig::api_url`), and the
dashboard writes what the shopper does. The library registers no page of its own
(route discovery belongs to the host); it offers queries, a guard, two components (a
price, a picture) and `GET {prefix}/status`. Details and the app are in
`docs/storefront.md`.

## Evolving events

Events are encoded with bitcode, which is positional: a stored event's layout, and
every type nested in it, is frozen once a deployment has written it. `events.lock` at
the workspace root records every event, every encoded type and every snapshotted view;
`dashboard/tests/events_lock.rs` fails when a line changes, so no shape changes by
accident. Nothing has been deployed yet, so the rules today are:

- A shape changes in place: edit the variant, the field or the enum, run
  `EVENTO_LOCK=force cargo test -p timada-dashboard --test events_lock` and commit the
  lock. Local stores holding the old shape are made afresh (`make reset`).
- Nothing is kept for data no deployment wrote: no old variant, no upcast, no
  fallback. The code reads as if written in one go.
- The read-model tables are one migration per crate (`0001`), edited in place the same
  way; `make reset` recreates the local database.

From the first production release on, a persisted variant, field or enum is never
edited or removed: add a variant, a companion event, or a new event with
`#[evento(upcast_to = New)]` on the old one, record it with `EVENTO_LOCK=update`, and
add a new read-model migration rather than editing one. A snapshotted view whose shape
changes bumps `.revision(n)`; its name is pinned with `name = "timada-<ctx>/<View>"` so
a move does not orphan snapshots.

## Configuration

`config/default.toml` is the full shape with its defaults; a file given with
`--config` (or `TIMADA_CONFIG`) overrides it, and `TIMADA__<SECTION>__<KEY>`
environment variables override both. Sections: `server` (URL, bind, region and the regions whose background
work this node also takes on, trusted proxies), `database` (the one file), `auth` (secret, session
lifetime, sign-in limits, OIDC providers), `mail` (the platform's server), `secrets`
(the one key the shops' secrets are sealed under; omit it and no shop can connect
payments, mail servers or shopper accounts), `media` (the pictures' directory and
public address, the default quota, the daily reconcile), `data_keys` (the personal-data key vault), `payments` (Stripe's
address, for the stand-in), `invoicing` (the VAT-number check, the euro reference
rates), `reviews`
(the review-request timer), `newsletter` (the send sweep), `automations` (the run
timer, the abandoned-checkout delay), `store` (the limits on what a storefront hands
in and the carts it starts), `carts` (how long an idle cart is kept), `invitations`
(their limits), `aliexpress` (the platform's app), `store_api`
(omit to not expose it), `cluster` (omit for a single node), `monitoring`.
`config/dev.local.toml` is the developer's own file and is ignored by git;
`config/e2e.toml` is what the browser suite runs on. Configuration is written for
operators: it names what things do, not the libraries behind them.

## The command line

`timada-dashboard serve` runs a node; `migrate` creates the database and applies
what is pending; `reset` (debug builds) drops and recreates it; `rejoin` marks the
store for a copy from a peer at the next start (see *The runtime*); `seed` loads a
catalogue snapshot into a tenant and prints a storefront's settings for it (see
`docs/storefront.md`); `snapshot save` keeps the platform (the database, pictures,
keys, and with `--also` a storefront's own file) in a directory, and
`snapshot restore` puts one back, every node of a cluster from copies taken together.
Everything but `serve` runs with the server stopped.

## Testing

- `cargo test --workspace` (inside `nix develop`): the crates' domain tests over a
  temporary store, and the dashboard's integration tests, which boot the runtime in
  process: `cluster` (three nodes in one process: a tenant's write lands on every
  node in that tenant only, a node that missed writes catches up, a node away too
  long is given up and copies the store back at its next start, a node at its write
  backlog refuses writes), `store_api` (TCP, keys, TLS),
  `storefront` (the library against a live store), `payments` (the webhook, the late
  payments and the refunds against a fake Stripe), `styles` (the stylesheet build),
  `events_lock`. The review endpoint is covered over HTTP by the browser suite.
- `make e2e`: `scripts/e2e.sh` builds the dashboard and the storefront, resets
  the database, starts the server on `config/e2e.toml` and runs Playwright through the
  compose stack (`make up`: Traefik for HTTPS, dex as an OIDC provider, MailDev as the
  mailbox the tests read codes from). Pass a spec path or a title filter to run part of it.
- `make e2e.cluster` (`E2E_NODES=3 scripts/e2e.sh`, which takes the same filters): the
  same suite on three nodes, each on `config/e2e.toml` with a `[cluster]` section added
  and its own ports and files (`data/e2e/node<N>`, its output in
  `data/e2e/node<N>.log`). Traefik sends each request to the next node, so a form's post
  and the page after it are answered by different nodes: the one setup where a read from
  a node's own file and a read through the cluster differ. The storefronts the specs
  start connect to node 0's store API, and their `/store/*` calls go through Traefik.
  The run fails when a node dies or is not ready at the end, as the other two would
  otherwise carry the suite without it. To pass on both setups, a spec reloads the page
  while it waits for something, and asserts on one load what belongs together: the next
  load may come from a node further behind.
- `make check` runs tests, clippy with warnings denied, rustfmt and cargo-machete.
