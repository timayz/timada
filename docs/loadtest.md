# Load testing

`make loadtest` (`scripts/loadtest.sh`) puts crowds of random shoppers on the
three-node cluster through the Chatounette clone, to find what breaks under
load and how much the platform takes before it slows down: the number to
know before thinking about sharding.

## What a run sets up

On a platform made afresh under `data/load` (the compose stack up, `make up`,
and the snapshot in `seeds/chatounette`):

1. three nodes (`config/load-node{0,1,2}.toml`, ports 3000–3002, store API
   4321–4323, consensus 7000–7002) behind Traefik, built in release unless
   `LOAD_PROFILE=dev`;
2. the shop, seeded through node 0 while nodes 1 and 2 serve, so its writes
   reach a quorum: `seed --owner-password` registers the owner without a
   mailed code, `--test-payments` links a test company and connects the
   Stripe test keys;
3. the storefront, through Traefik's balancer over the nodes' store API, with
   its own copy of the catalogue in `data/load/chatounette.db`;
4. `timada-loadtest`, which also serves the stand-in for Stripe the nodes
   call (`payments.stripe_api_url`), in front of stripe-mock.

Seeding takes about 35 minutes and the storefront's copy of the catalogue about
10. Every run leaves carts, orders and invoices behind, and a platform with more
history is slower, so two runs over the same files do not compare. Hence:

- `LOAD_SNAPSHOT=1` keeps the platform as it is once seeded and the catalogue
  copy is in, in `data/load-snapshot`: one `timada-dashboard snapshot save` per
  node with the servers stopped (every database whole, the three nodes' copies
  agreeing; node 0's also holds the storefront's database and the seed's
  settings), then the servers start again for the run.
- `LOAD_REUSE=1` starts from that copy (`snapshot restore --all-nodes` on every
  node), in seconds, so every run starts from the
  same state: change one thing, run again, and the difference is that change.
  Without a copy it starts again over what the last run left (to measure how the
  platform ages, on purpose).

```
LOAD_SNAPSHOT=1 make loadtest ARGS="ramp --max 3"   # once: seed, keep, short run
LOAD_REUSE=1 make loadtest ARGS="ramp --keep-going --max 60"
```

## What a shopper does

Visits arrive at random (a Poisson process) at the step's rate whatever the
server's pace, as shoppers do; each runs on its own, its choices drawn from
its seed (`--seed` replays a run). A visit lands on the home page, a category,
a product or a search; reads a few more pages (`--pages`), thinking between
them (`--think-secs`); types in the search box (the dropdown asks at every
letter); puts products in the cart (`--add-to-cart`), picks a variant left
to sell; changes the cart or tries the promo code now and then; checks out
(`--checkout`), from France, Belgium or Switzerland, and mostly pays
(`--pay`). Paying makes the stand-in tell the dashboard through the shop's
webhook a moment later, retrying on 503 and now and then twice; the shopper
reloads the return page until it sends on to the order.

## Modes

- `ramp --start 1 --factor 1.5 --step-secs 60`: rates rising until a step
  breaks a limit: p99 (`--page-p99-ms` for the pages, `--action-p99-ms` for the forms), failed requests
  (`--error-pct`), paying to the order showing (`--order-p99-ms`), a checkout
  the dashboard did not answer in the storefront's wait, or visits dropped
  because `--max-visits` are under way. The last step held is the capacity.
- `soak --rate 5 --secs 1800`: one rate, a report line every `--every`
  seconds, for leaks, backlogs and drift (watch the memory and the
  write-ahead logs grow across lines).

## Reading the report

A line per step: the rate, requests per second, p50/p99, failures, orders
shown and their p99, visits under way, the processes' CPU (cores) and memory.
Then the slowest routes, the journey's counters (`cart.refused.*`,
`checkout.refused.*`, `webhook.retry`, ...) and the anomalies, each with
samples. The whole of it is in `data/load/report.json`, the logs beside it.

An anomaly is something that should not happen whatever the load, and makes
the run exit with 2:

| Kind | Means |
|---|---|
| `amount_mismatch` | Stripe was asked for another amount than the checkout page showed |
| `order_missing` | paid, and no order a minute later |
| `order_not_paid` | the order page shows an order that is not paid |
| `payment_bounced` | back from paying, sent somewhere other than the order |
| `refund` | the dashboard refunded a payment (it does when it cannot place the order) |
| `webhook_refused` / `webhook_gave_up` | the dashboard turned a genuine payment's webhook away, or never took it |
| `cart_lost` | products added, the cart page shows none |
| `checkout_refused` | a refusal a seeded shop should never give |
| `visits_stuck` | visits still under way long after the last step |
