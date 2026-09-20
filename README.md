# Timada

An e-commerce framework in Rust: the domain of a shop — catalogue, prices,
stock, carts, orders, payments, shipping, invoices, returns, promotions,
reviews — as event-sourced bounded contexts you assemble into your own
application, with a back-office you mount into it.

Think "Medusa.js in Rust": libraries, not a hosted product. You own the
binary, the database and the storefront.

- **Event sourcing / CQRS** with [evento] over SQLite. Every context is a
  crate: aggregates and their events, commands, projections, SQL read models.
- **A mountable admin**, written with [topcoat], that you serve under any path
  of a topcoat or axum/tower app.
- **A demo storefront** showing how a host wires it all together.

> Pre-1.0. The shapes of stored events are already treated as permanent — see
> [Evolving what timada persists](docs/event-evolution.md) — but crate APIs
> still move.

## Try it

The dev shell (`nix develop`, or direnv) provides Rust, Tailwind and the
`topcoat` CLI.

```sh
cargo run -p demo -- --seed     # sample catalogue, a customer, an order, the accounts below
topcoat dev -p demo             # bundles assets, watches, serves on http://127.0.0.1:3000
```

| | URL | Login |
|---|---|---|
| Storefront | `/` | `jonathan@example.com` / `demo1234`, or create an account |
| Admin | `/admin` | `admin@timada.example` / `admin` |

Without an asset bundle (`cargo run -p demo`) the storefront works and the
admin renders unstyled.

Things worth trying: order something to a metropolitan address, then to
Martinique (the checkout switches to prices without French VAT and another
carrier), then to Berlin (`DE`: German VAT replaces the French one); capture the payment from the admin (the demo has no payment provider: the payment step of the checkout waits for it) and ship the order; refund part of it from the order page; ask for a
return from the account; open the invoice and download its PDF; look at `/admin/emails` to see what
the shop would have sent.

Environment of the demo:

| Variable | Default | |
|---|---|---|
| `TIMADA_BASE_URL` | `http://127.0.0.1:3000` | links in e-mails, and where the payment provider sends shoppers back |
| `TIMADA_MAIL_FROM` | `Timada demo <no-reply@timada.example>` | |
| `TIMADA_SMTP_URL` | — | with `--features smtp`, e-mails are sent instead of logged |
| `TIMADA_PAYMENT_TIMEOUT_SECS` | `1800` | an unpaid order is cancelled and its stock released |
| `TIMADA_STRIPE_SECRET_KEY`, `TIMADA_STRIPE_PUBLISHABLE_KEY`, `TIMADA_STRIPE_WEBHOOK_SECRET` | — | with `--features stripe`, shoppers pay by card on the payment step and refunds go back through Stripe |

To pay for real (in Stripe's test mode): build with `--features stripe`, set the
three keys, and let Stripe reach the webhook —
`stripe listen --forward-to 127.0.0.1:3000/webhooks/stripe` prints the
`whsec_…` to use. The card `4242 4242 4242 4242` pays, `4000 0027 6000 3184`
asks for 3-D Secure.

## The crates

Bounded contexts live in `crates/`, one crate each, package `timada-<context>`.

| Crate | What it owns |
|---|---|
| `timada-core` | `Money`, `Address`, derived ids, formatting, the SQLite test helper |
| `timada-catalog` | products — description, specs, media, archiving — and the category tree they are filed under; product and category lists |
| `timada-pricing` | listed price (tax-inclusive) with its VAT rate, eco-participation, instalment offers |
| `timada-inventory` | stock per product and location, reservations, returned stock, back-in-stock alerts |
| `timada-customer` | customers, their e-mail, billing and delivery addresses; customer list |
| `timada-cart` | carts: lines with a price snapshot, promo code, saved carts, the checkout fact |
| `timada-promotion` | promo codes (capped redemptions) and vouchers (balances) |
| `timada-payment` | the payment of an order: requested, captured, declined, refunded — and the `PaymentProvider` port the money moves through, with a Stripe adapter behind the `stripe` feature |
| `timada-shipping` | delivery methods and the shipment of an order |
| `timada-tax` | **library, no events**: tax zones, what is charged in a zone, VAT per rate |
| `timada-order` | the order, the checkout ACL that places it, the fulfillment saga, payment timeouts |
| `timada-invoice` | one invoice per order, legal numbering, credit notes, the invoice as a document — and, with the `pdf` feature, as a PDF file |
| `timada-returns` | returns (RMA) of shipped orders: request, review, reception, restock and refund |
| `timada-review` | product reviews (moderated) and questions & answers |
| `timada-mailer` | transactional e-mails (with attachments) through a SQL outbox and pluggable transports; feature `invoice-pdf` e-mails each issued invoice as a PDF |
| `timada-admin` | the mountable back-office over all of the above |

`tools/event-lock` keeps the persisted shapes append-only, and `demo/` is the
example host.

How they fit together, what a host has to wire, and the conventions every
crate follows: **[docs/architecture.md](docs/architecture.md)**.

## Working on it

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo machete
```

After adding an event, a value type used by an event, or a snapshotted view:

```sh
cargo run -p timada-event-lock -- update     # then commit events.lock
```

Commits follow [Conventional Commits](https://www.conventionalcommits.org/).

## Documentation

- [Architecture](docs/architecture.md) — contexts, dependencies, the life of an
  order, host wiring, conventions.
- [Evolving what timada persists](docs/event-evolution.md) — what is permanent,
  `events.lock`, and what to do instead of editing an event.
- [Mounting the admin](crates/admin/README.md).

## License

MIT.

[evento]: https://github.com/timayz/evento
[topcoat]: https://github.com/tokio-rs/topcoat
