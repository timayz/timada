# timada-admin

A [topcoat](https://github.com/tokio-rs/topcoat) admin for the timada contexts
(orders, products, categories, families, stock, customers, promotions, invoices, VAT, refunds, returns,
reviews, questions, e-mails) that you mount into your own app — a topcoat app or any tower/axum
app.

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
`timada_catalog::category_list_subscription`,
`timada_catalog::family_list_subscription`,
`timada_inventory::stock_list_subscription`,
`timada_customer::customer_list_subscription`,
`timada_promotion::code_list_subscription`,
`timada_invoice::invoice_list_subscription`,
`timada_invoice::credit_note_list_subscription`,
`timada_invoice::vat_journal_subscription`,
`timada_payment::refund_list_subscription`,
`timada_returns::return_list_subscription`,
`timada_review::review_list_subscription` and
`timada_review::question_list_subscription`, each with `.data(pool)` — and
their migrations (`timada_payment::migrations()` is new with the refunds
section).

Refunds are asked for from an order's page once its payment is captured, in
one or several goes up to the captured amount — and by the fulfillment saga
when a paid order is cancelled, or by a return. A refund is only *made* once
the payment provider confirmed it (the host runs
`timada_payment::refund_execution_subscription` and
`timada_payment::run_provider_refunds`): until then it shows as pending on the
order page and at the top of the refunds section; one the provider refused can
be asked for again, or settled by hand with the reference of the transfer that
replaced it. The refunds section is the journal of the refunds made. Invoices are
read-only: orders drive their lifecycle, and every refund is documented by a
credit note shown under its invoice, provided the host runs
`timada_invoice::credit_notes_from_refunds_subscription`.

The orders section has a queue of its own, *À expédier*: the paid orders
waiting for their parcel, the one waiting longest first, with where each goes;
those waiting longer than `AdminConfig::ship_within` are flagged, and counted
on the orders page. The fulfillment saga never times a paid order out —
somebody ships it.

With an archive (`AdminServices::new(..).with_archive(archive)`), an invoice's
page shows what was filed — date, size, SHA-256, whether it was reconstituted —
checks the file against that hash on demand, and its PDF download serves the
archived file rather than a fresh rendering.

The VAT section reads a quarter out of the issued invoices and the credit
notes: the shop's own VAT by rate, the one-stop-shop (OSS) return by member
state and rate with the corrections of earlier quarters, the intra-community
supplies to businesses of other member states (by buyer VAT number), exports —
and the OSS return as a CSV file.

The categories section is the shop's tree: opening a category (its address is
definitive, its name is not), renaming, describing, moving a branch, ranking it
among its siblings, archiving it — and the filters of its listing: which lines
of the technical sheet shoppers filter it by (`Groupe > Libellé`, a line
each), picked from the specs its products actually have; a category without a
list of its own goes by its parent's. Products are filed from their own page,
or when they are created; the same page edits their technical sheet
(`Groupe | Libellé | Valeur`, a line each).

The families section gathers the products that are one article in several
versions: a family says what tells them apart (`Couleur : Noir, Argent`, a
line per option, the values in the order shoppers see them) and products take
their place in it by reference (SKU). Each version stays a product — priced,
stocked and edited from its own page, which shows its family and lets it leave.
A value a variant stands on cannot be removed; a family dissolves once empty.

Reviews wait in the reviews section until an operator publishes or rejects
them; only published ones reach the storefront and the product rating. Product
questions are moderated too — a question is published, refused, or published
by answering it as the shop — and so is every answer a customer gives to a
published question.

An issued invoice has a print view (`…/invoices/{id}/print`): the admin's
header is hidden on paper, so the browser's print dialog gives the PDF. It
prints the seller's identity from `AdminConfig::invoice_issuer`.

The returns section is where a customer's return is reviewed (accept or
refuse) and, once the parcel is in, received: what is taken back line by line,
whether it goes into stock again, and whether the customer gets their money or
store credit. Restocking and refunding are then done by
`timada_returns::return_processing_subscription`, which the host must run.

The e-mails section reads `timada-mailer`'s outbox (`timada_mailer::migrations()`):
what was written to customers, what waits for the delivery worker, and what
the relay refused — with a retry for the ones it gave up on.

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
