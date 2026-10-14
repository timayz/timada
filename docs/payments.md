# Payments

Shoppers pay by card on the merchant's own Stripe account. An order exists only once
Stripe has confirmed the payment to the dashboard. How it works is in
`docs/architecture.md` (*Payments*); the storefront side is in `docs/storefront.md`
(*Cart and checkout*). This page covers setting it up and testing it locally.

## The flow in one screen

1. The storefront asks to pay for a cart (`start_payment`).
2. The dashboard prices the cart, opens a PaymentIntent with the tenant's Stripe key and
   hands back what the card form needs. It refuses instead when the price moved since
   the shopper saw it, an item is off sale or Stripe is not set up.
3. The shopper's browser confirms the payment in Stripe's Payment Element
   (`/checkout/pay` in the example app).
4. Stripe calls `POST /stripe/webhooks/{tenant_id}`. The dashboard checks the signature,
   reads the payment back from Stripe and creates the order.
5. The browser, back on `/checkout/complete`, waits for the order and shows it.

Cancelling an order in the dashboard refunds it through Stripe first.

## Setting it up

- **Operator.** A `[payments]` section in the dashboard config enables payments.
  Without it, every request to pay is refused.

  ```toml
  [payments]
  # At least 32 characters, the same on every node. It encrypts the tenants' Stripe
  # secrets: changing it makes them unreadable, and merchants enter them again.
  secret_key = "…"
  # stripe_api_url = "http://localhost:12111"   # tests only; Stripe itself when unset
  ```

- **Merchant, first.** On `/t/<slug>/invoicing` (permission `tenant:manage`), the
  legal identity every invoice names: legal name and form, share capital, SIRET,
  RCS city, registered address, and either the VAT number (with whether it is
  registered for the EU One-Stop Shop) or nothing (the exemption for small
  businesses, art. 293 B du CGI). Until it is set, every request to pay is refused
  with `invoicing_not_configured`. A VAT-registered merchant then sets each
  product's kind of goods (standard, books, food, medicines, press), which decides
  its rate in each country; catalog prices include VAT.

- **Merchant.** On `/t/<slug>/payments` (permission `payments:manage`):
  1. Paste the publishable key and the secret key from Stripe (Developers › API keys).
     The secret key is checked against Stripe before it is saved.
  2. In Stripe (Developers › Webhooks), add an endpoint at the URL the page shows,
     `https://<dashboard>/stripe/webhooks/<tenant id>`, for the event
     `payment_intent.succeeded`.
  3. Paste that endpoint's signing secret (`whsec_…`).

- **Storefront.** Its API key needs `store:read` and `store:checkout`.

## Testing locally with Stripe's test mode

This drives the real card form, so you need a Stripe account in test mode, the Stripe
CLI and an internet connection.

1. **Turn payments on.** Add `[payments]` with a `secret_key` to `config/dev.local.toml`
   and restart `topcoat dev`. Leave `stripe_api_url` unset.

2. **Forward Stripe's webhooks to your machine:**

   ```sh
   stripe login   # the Stripe CLI comes with `nix develop`
   stripe listen --events payment_intent.succeeded \
     --forward-to http://localhost:3000/stripe/webhooks/<tenant_id>
   ```

   - `<tenant_id>` is the last segment of the webhook URL shown on `/t/<slug>/payments`.
   - Forward to `localhost:3000` rather than `https://timada.localhost`, so the CLI does
     not trip over the mkcert certificate.
   - Keep it running while you test, and copy the signing secret (`whsec_…`) it prints.

3. **Connect the account.** On `/t/<slug>/payments`, enter:
   - `pk_test_…` and `sk_test_…` from Stripe's test mode;
   - the `whsec_…` printed by `stripe listen`, not one from an endpoint registered in
     the Stripe dashboard.

4. **Give the storefront a key that can sell.** Tick `store:checkout` on its key on the
   API keys page, or mint a new one. If the key changes, remove the replica first:
   `rm storefront.db*`, then `make storefront.migrate`.

5. **Buy something.** Run the storefront (`make storefront` with `STORE_TOKEN` and
   `STORE_MEDIA_URL`), add a product, check out and pay with a test card. Use any
   future expiry date and any CVC.

   | Card | What happens |
   |---|---|
   | `4242 4242 4242 4242` | The payment succeeds. |
   | `4000 0025 0000 3155` | 3-D Secure is asked for; approve it in the popup. |
   | `4000 0000 0000 9995` | The payment is declined; the error shows under the form. |

   You land on "Confirming your payment…", then on the order once the webhook arrives.
   `stripe listen` logs a `204` for it.

6. **Check the merchant's side.**
   - `/t/<slug>/orders` lists the order as Paid.
   - *Mark as fulfilled* takes an optional tracking number.
   - *Cancel and refund* refunds the order; the refund shows in Stripe's test dashboard
     under Payments.

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

## Automated tests

These run without a Stripe account and cover everything except typing a card into
Stripe's form.

- `cargo test -p timada-dashboard --test payments` drives the dashboard's side against a
  fake Stripe:
  - a paid cart becomes one order, even when the webhook is sent twice;
  - forged, stale and unrelated webhooks change nothing;
  - refusals;
  - a second payment of a cart is refunded;
  - the secrets are sealed.
- With `docker compose up -d stripe-mock`, setting
  `STRIPE_MOCK_URL=http://localhost:12111` also sends the real Stripe client's calls to
  stripe-mock. The test is skipped without it.
- `make e2e` runs the whole flow in a browser. It uses `tests/e2e/stripe.ts`, a stand-in
  in front of stripe-mock that remembers what stripe-mock forgets: the metadata, and
  whether a payment was paid. The flow it runs:
  1. connect Stripe;
  2. check out and reach the card form;
  3. send a signed `payment_intent.succeeded`;
  4. see the paid order, fulfil it, then cancel and refund another.

  It needs stripe-mock running, and `topcoat dev` stopped, since they share ports.
