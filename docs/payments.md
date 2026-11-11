# Payments

Shoppers pay by card on the merchant's own Stripe account. Checking out only holds the
money on the shopper's card (Stripe's manual capture); the order exists from then on,
and the money is taken later: at once for goods the shop holds, once the supplier has
the goods for goods it buys after the sale (AliExpress). How it works is in
`docs/architecture.md` (*Payments*); the storefront side is in `docs/storefront.md`
(*Cart and checkout*). This page covers setting it up and testing it locally.

## The flow in one screen

1. The storefront asks to pay for a cart (`start_payment`).
2. The dashboard prices the cart, opens a PaymentIntent with the tenant's Stripe key,
   held only (`capture_method=manual`), and hands back what the card form needs. It
   refuses instead when the price moved since the shopper saw it, an item is off sale or
   Stripe is not set up. It also notes who supplies the goods: a line the shop buys on
   AliExpress once the order is placed makes the order wait for it. The payments
   subscription only queues the request on the node; the node's lanes answer up to 16
   carts at once, a cart's requests one after the other, and the first node of the
   region to claim a request (`Cart::CheckoutClaimed`) is the one that answers it. A
   fault of the store's puts the request back for a growing while instead of holding
   the subscription for every shop; `/admin/cluster` shows how many are being answered.
3. The shopper's browser confirms the payment in Stripe's Payment Element
   (`/checkout` in the storefront), which holds the money on the card.
4. Stripe calls `POST /stripe/webhooks/{tenant_id}` (`payment_intent.amount_capturable_updated`).
   The dashboard checks the signature, reads the payment back from Stripe and creates the
   order, its money held (`Authorized`).
5. The money is taken (`Order::CaptureRequested`, then Stripe, then `Order::Paid`, and the
   invoice): right away when the shop holds every good, or once the AliExpress order is
   placed, or when the merchant clicks *Take the payment*.
6. The browser, back on `/checkout/complete`, waits for the order and shows it: paid,
   for goods the shop holds; saved, for goods a supplier buys first.

The shopper is mailed at each step that concerns their money: the confirmation once the
order exists (saying when the card is only held), the payment taken once the invoice is
issued, and the cancellation, saying whether money goes back (`docs/mail.md`).

Only payment methods that can hold money are offered: cards, Apple Pay, Google Pay,
Link, PayPal, Klarna. SEPA debit, iDEAL and Bancontact cannot, and leave the form.

The goods and the discount's use are held from step 2 for thirty minutes. A payment
confirmed after that (a card form left open, a slow bank, a webhook delivered late) has
the dashboard take the holds again before it creates the order: nothing changes while
they stand, and what is left is taken otherwise. When the last of it went to someone
else in between, the cart records that the payment goes back (`Cart::PaymentReturnRequested`),
the hold is given back, the cart records that it went (`Cart::PaymentReturned`) and the
storefront tells the shopper (`payment_returned_*` codes) instead of waiting for an
order that will not come. Nothing is charged. A second payment of a cart that is an
order already goes back the same way; one Stripe did not answer for is given back by the
sweep a minute later.

Most card forms left open never get that far: every `cancel_check_secs` (a minute by
default) each node sweeps the checkouts whose payment is still unconfirmed once their
hold lapsed and cancels the payment with Stripe (`Cart::PaymentCancelled`), a bank
challenge left open included; the storefront reads it as `payment_expired` and the
shopper asks to pay again, nothing having been charged. A payment Stripe is settling is
left alone and asked about again later. One the shopper confirmed whose webhook never
came (lost, or the endpoint does not send that event) is turned into its order by the
sweep, as the webhook would.

A card holds the money for a week. Stripe drops a hold nobody took
(`payment_intent.canceled`): the order is cancelled, nothing taken, its goods back on
the shelf. The order page shows until when the card holds it, and the AliExpress
forwarder does not place an order whose hold ends within twelve hours. A hold dropped
once AliExpress may have the order (`Order::SupplierOrderStarted`, no cancellation
asked) cancels nothing, as AliExpress may be shipping it: the order records the hold
gone (`Order::PaymentReleased` alone) and stays open for the merchant, who settles it
with the shopper and cancels it, saying they cancelled it on AliExpress too. That
cancellation asks Stripe nothing, as nothing is held any more, and it is settled even
once the shop took its Stripe keys away.

The shopper may cancel an order of goods a supplier buys after the sale until the
shop asks the supplier for them (`/store/orders/{order_id}/cancel`): its hold goes back,
nothing is charged, and a discount code's use returns. An own-stock order is paid at once
and is the merchant's to cancel.

Cancelling an order settles the money with Stripe first: the order records the
cancellation (`Order::CancellationRequested`), then a hold is given back (written with
`Order::Cancelled`), or money taken is refunded in full, the refund recorded first
(`Order::RefundRequested`, which fixes the idempotency key), then made, recorded
(`Order::Refunded`), and only then is the order cancelled. If Stripe refuses or does not
answer, the cancellation stands, the page offers to retry under the same key, and the
sweep settles it otherwise, so Stripe never refunds twice. A capture or a cancellation
Stripe could not be asked about at once is settled by the same sweep after a minute. If
Stripe took a refund but the recording failed here, or a refund was made by hand in
Stripe's dashboard, the `charge.refunded` webhook records what Stripe holds and the
order does not know yet, and the credit note follows.

## Setting it up

- **Operator.** A `[payments]` section in the dashboard config enables payments.
  Without it, every request to pay is refused.

  ```toml
  [payments]
  # At least 32 characters, the same on every node. It encrypts the tenants' Stripe
  # secrets: changing it makes them unreadable, and merchants enter them again.
  secret_key = "…"
  # stripe_api_url = "http://localhost:12111"   # tests only; Stripe itself when unset
  # cancel_check_secs = 60   # how often lapsed, unconfirmed payments are cancelled
  ```

- **Merchant, first.** The tenant sells as a company: on `/t/<slug>/invoicing`
  (permission `tenant:manage`), link it to a company the member owns, or register
  one. See `docs/invoicing.md`. Until then, every request to pay is refused with
  `invoicing_not_configured`; a sale to a consumer in another EU country in a
  currency the European Central Bank publishes no euro rate for is refused with
  `oss_required_for_currency` unless the company ticked the One-Stop Shop (one in
  dollars, pounds or another published currency is counted against the EU
  threshold at the Bank's rate of the day, see `docs/invoicing.md`).

- **Merchant.** On `/t/<slug>/payments` (permission `payments:manage`):
  1. Paste the publishable key and the secret key from Stripe (Developers › API keys).
     The secret key is checked against Stripe before it is saved.
  2. In Stripe (Developers › Webhooks), add an endpoint at the URL the page shows,
     `https://<dashboard>/stripe/webhooks/<tenant id>`, for the events
     `payment_intent.amount_capturable_updated`, `payment_intent.succeeded`,
     `payment_intent.canceled` and `charge.refunded`.
  3. Paste that endpoint's signing secret (`whsec_…`).

  "Stop taking payments" stops new checkouts at once: storefronts stop offering the card
  form and new requests to pay are refused. The keys stay, sealed, while anything is held
  or owed (an order's money held or being taken, cancelled or refunded, a card form still
  open, a request being answered, a payment being given back), so the sweeps and the
  webhook settle it; the page says how many payments are left. The keys stay until the
  merchant forgets them ("Forget the keys"), which is refused before the stop is eight
  days old (a card holds money for up to seven) and while anything is still counted as
  held or owed. Saving keys again takes payments again. Meanwhile another Stripe account
  is refused: another publishable key, or a secret key that cannot read a payment still
  held. A new secret key of the same account is fine.

- **Storefront.** Its API key needs `store:read` and `store:checkout`.

## Testing locally with Stripe's test mode

This drives the real card form, so you need a Stripe account in test mode, the Stripe
CLI and an internet connection.

1. **Turn payments on.** Add `[payments]` with a `secret_key` to `config/dev.local.toml`
   and restart `topcoat dev`. Leave `stripe_api_url` unset.

2. **Forward Stripe's webhooks to your machine:**

   ```sh
   stripe login   # once; the Stripe CLI comes with `nix develop`
   make stripe.listen
   ```

   - It forwards the four payment events of the payments page to the shop `make seed`
     filled, whose id the seed wrote to `config/storefront.local.env`. For another shop, use
     `STORE_TENANT_ID=<tenant_id> make stripe.listen`: `<tenant_id>` is the last
     segment of the webhook URL shown on `/t/<slug>/payments`.
   - It forwards to `localhost:3000` rather than `https://timada.localhost`, so the CLI
     does not trip over the mkcert certificate.
   - Keep it running while you test, and copy the signing secret (`whsec_…`) it prints.

3. **Connect the account.** On `/t/<slug>/payments`, enter:
   - `pk_test_…` and `sk_test_…` from Stripe's test mode;
   - the `whsec_…` printed by `stripe listen`, not one from an endpoint registered in
     the Stripe dashboard.

   To skip this after each `make seed`, put them in
   `config/stripe.local.toml`, which git ignores. The seed then connects the shop from
   that file, over any account it has. It checks
   the keys' prefixes only, so a key Stripe refuses shows up at the first payment:

   ```toml
   publishable_key = "pk_test_…"
   secret_key = "sk_test_…"
   webhook_secret = "whsec_…"
   ```

4. **Give the shop a key that can sell.** The key `make seed` wrote can. For another
   key, tick `store:checkout` on it on the API keys page. If it is another tenant's,
   remove the replica first, or the shop keeps showing the old tenant's products:
   `rm storefront.db*`, then `make storefront.migrate`.

5. **Buy something.** Run the shop (`make storefront`), add a product, check out and pay with a test card. Use any
   future expiry date and any CVC.

   | Card | What happens |
   |---|---|
   | `4242 4242 4242 4242` | The payment is held, then taken. |
   | `4000 0025 0000 3155` | 3-D Secure is asked for; approve it in the popup. |
   | `4000 0000 0000 9995` | The payment is declined; the error shows under the form. |

   You land on "Confirming your payment…", then on the order once the webhook arrives
   and, for goods the shop holds, the money is taken. `stripe listen` logs a `204` for
   each event.

6. **Check the merchant's side.**
   - `/t/<slug>/orders` lists the order as Paid (Authorized for goods a supplier buys:
     *Take the payment* takes it).
   - *Mark as fulfilled* takes an optional tracking number and, if the merchant wants the
     shopper to open it, the carrier's page for it; the order's pages show the number
     either way and link nowhere else.
   - *Cancel and refund* refunds a paid order; the refund shows in Stripe's test
     dashboard under Payments. *Cancel and release the hold* cancels an order whose
     money is only held: Stripe shows the payment cancelled, nothing taken.

### When it stalls

- **The page stays on "Confirming your payment…".** The webhook did not land. Check
  that `stripe listen` is running, that its URL carries the right tenant id, and that the
  saved signing secret is the one it printed. The dashboard answers a bad signature
  with `400` and logs it.
- **"Payments are not available right now" at checkout.** The dashboard refused to open
  a payment for one of these reasons, and its log says which:
  - `[payments]` is missing from the config;
  - the tenant has no Stripe account connected;
  - Stripe refused the call.
- **Keys are refused on `/t/<slug>/payments`.** A secret key starts with `sk_` (or `rk_`
  for a restricted key), a publishable key with `pk_`, and a signing secret with
  `whsec_`.

### Round-trip checklist

Run this once with real test keys whenever the payment flow changes, and before
turning payments on for a merchant. Follow the steps above, then tick each line.

- [ ] **Payment Element.** The card form shows on `/checkout`, and `4242 4242 4242
      4242` goes through. A declined card (`4000 0000 0000 9995`) shows its error under
      the form, and 3-D Secure (`4000 0025 0000 3155`) opens its popup.
- [ ] **Webhook.** `stripe listen` logs a `204` for
      `payment_intent.amount_capturable_updated` and then `payment_intent.succeeded`, and
      the page moves on from "Confirming your payment…".
- [ ] **Order.** `/t/<slug>/orders` lists it as Paid at the amount Stripe's test
      dashboard shows for the payment, its invoice is issued, and the shopper gets the
      confirmation and the "Payment received" mail.
- [ ] **Refund on cancel.** *Cancel and refund* marks the order Cancelled. Stripe's test
      dashboard shows the payment refunded in full, and a credit note is issued.
- [ ] **Hold given back.** An order whose goods a supplier buys stays Authorized, and
      Stripe's test dashboard shows its payment Uncaptured. *Cancel and release the hold*
      marks it Cancelled; Stripe shows the payment cancelled, no invoice is issued, and
      the shopper's "Cancelled" mail says nothing was charged.
- [ ] **Refund in Stripe.** Refund part of a payment by hand in Stripe's test dashboard.
      `stripe listen` logs a `204` for `charge.refunded`, and the refund reaches the order
      with its credit note.

Last run:

| Date | Who | Result | Notes |
|---|---|---|---|
| | | | |

## Live test

`a_checkout_goes_through_stripe_test_mode` in `dashboard/tests/payments.rs` runs the
dashboard's side of the round trip against Stripe itself, without a browser:

```sh
STRIPE_TEST_SECRET_KEY=sk_test_… cargo test -p timada-dashboard --test payments -- --ignored
```

- It is ignored by default, since it needs the network and a Stripe account. Run with
  `--ignored` but without the variable (or with it empty), it does nothing.
- It refuses any key that is not a test-mode key (`sk_test_…` or `rk_test_…`).
- What it does:
  1. checks the key, as the payments page does;
  2. checks out a cart, so the payments subscription opens a real PaymentIntent, held
     only;
  3. confirms it with Stripe's test card `pm_card_visa`, as the Payment Element would;
  4. sends the webhook a signed `payment_intent.amount_capturable_updated`. The
     dashboard reads the payment back from Stripe, so the order is placed only if Stripe
     holds the money;
  5. waits for the money to be taken (the shop holds the goods) and checks the order is
     Paid at 25.00 EUR, with the card's deadline;
  6. cancels it with a refund, and asks Stripe whether the refund succeeded.
- The webhook is signed by the test itself, so it does not cover Stripe delivering
  one. The checklist's `stripe listen` step does.
- It leaves one refunded 25.00 EUR payment in Stripe's test dashboard per run.

## Automated tests

These run without a Stripe account (the live test above aside) and cover everything
except typing a card into Stripe's form.

- `cargo test -p timada-dashboard --test payments` drives the dashboard's side against a
  fake Stripe:
  - a confirmed cart becomes one order, even when the webhook is sent twice, and its
    money is taken once;
  - forged, stale and unrelated webhooks change nothing;
  - refusals, a refused capture, a hold given back on cancel, a lapsed hold;
  - a second payment of a cart is given back;
  - what Stripe did not answer is settled by the sweep;
  - the secrets are sealed.
- With `docker compose up -d stripe-mock`, setting
  `STRIPE_MOCK_URL=http://localhost:12111` also sends the real Stripe client's calls to
  stripe-mock. The test is skipped without it.
- `make e2e` runs the whole flow in a browser. It uses `tests/e2e/stripe.ts`, a stand-in
  in front of stripe-mock that remembers what stripe-mock forgets: the metadata, and
  whether a payment was confirmed, taken or given back. The flow it runs:
  1. connect Stripe;
  2. check out and reach the card form;
  3. send a signed `payment_intent.amount_capturable_updated`;
  4. see the paid order, fulfil it, then cancel and refund another.

  It needs stripe-mock running, and `topcoat dev` stopped, since they share ports.
