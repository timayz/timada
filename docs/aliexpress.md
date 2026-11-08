# AliExpress

A shop imports products from AliExpress's dropshipping catalogue through the
**platform's** AliExpress app, which acts for the AliExpress buyer account of the
company the shop sells as. The dashboard keeps each imported product in step with
AliExpress: its stock, its price when a rule prices it, and whether AliExpress still
sells it.

## Setting it up (operator, once)

1. On AliExpress's open platform, create an app with access to the dropshipping API.
   AliExpress has to approve that access before product calls work.
2. Register `/aliexpress/callback` under `server.url` as the app's callback, and
   `/aliexpress/webhook` as its message push callback with the topic
   `DROPSHIPPER_ORDER_STATUS_UPDATE` ticked (the App Console's notification tab).
   AliExpress only pushes to an HTTPS address with a CA-issued OV or EV certificate.
3. Put the app's key and secret in `[aliexpress]` (`app_key`, `app_secret`), with
   `secrets.key` set so the tokens can be sealed. Without both, nobody can connect.

AliExpress counts every call against the one app, whichever company it acts for. Read
the app's daily quota in the open platform's console and set `calls_per_day` from it,
leaving a margin (see *The day's calls*).

## Connecting the account (company owner)

On the company's page (`/companies/{id}`), press *Connect the AliExpress account*.
AliExpress asks the buyer account to say yes to the app, then sends the owner back.
The tokens are kept sealed on the company (`Company::AliexpressAuthorized`). Every
tenant selling as the company imports through that account and places its orders
with it; *Disconnect* forgets it for all of them.

Connecting the same buyer account again, from the same or another company, gives a
new token: AliExpress may retire the earlier one, so one buyer account belongs to one
company.

## Setting up a shop (merchant)

On `/t/{slug}/aliexpress`, for members with `aliexpress:manage`:

1. The shop must sell as a company (linked on its Invoicing page) whose account is
   connected. The page says which of the two is missing.
2. Choose what the shop buys in:
   - the country goods ship to;
   - the currency, one the shop sells in;
   - the language products are worded in.
3. Keep or change the **price rule**: multiply the cost, add an amount, and round to
   the cent, up to the next .90, up to the next .99, or up to a whole amount. A rule
   never goes below cost, and what it gives is the catalog price, VAT included. Blanking
   every row prices by hand: the import uses the cost as the starting price and later
   reads leave prices alone.

   Cheap items usually bear a higher rate than dear ones, so the rule can go by
   **cost band**: each row prices costs up to its amount (inclusive) that no lower
   band took, and the last row prices any higher cost. Up to ten bands; each has its
   own multiplier, amount added and rounding. A blank row is dropped, and two bands
   ending at the same amount are refused. A product's own rule (on its page) is one
   rate for every cost.

   A price never falls below the price at the end of a lower band, so a supplier's
   one-cent rise cannot cut it at the next read. With ×2.5 up to 25.00 and ×2.2 above,
   a cost of 25.01 stays at 62.50 until ×2.2 catches up, at about 28.41. A price held
   this way still takes its own band's rounding.

   A tenant is created with the usual rates for dropshipping as its rule, each rounded
   up to the next .90: ×3 up to 10.00, ×2.5 up to 25.00, ×2.2 up to 50.00, ×1.8 up to
   100.00, ×1.5 up to 250.00, ×1.3 above.

   The **price tolerance** (up to 20 %, blank by default) leaves a price alone while
   what the rule gives is within that much of it, so a small move in the cost neither
   changes the price nor stops a shopper at checkout. It is measured from the price as
   it stands, so small moves that add up are not each let through. The shop absorbs
   the difference until the rule's price leaves the tolerance. *Sync now* always gives
   the rule's price.

*Stop importing* forgets what the shop buys and how its orders go. The imported
products stay as they are, and so do the price rule and the company's account.

## Importing

Paste a product's number or address and press *Import*. The dashboard fetches the
product in the background, on the first node to take it on. It then fills a **draft**:

- the name, the description as plain text, the facts, and up to twenty pictures,
  including each colour's own;
- one option per AliExpress property and one variant per priced combination, the
  AliExpress sku id as its code;
- each variant's stock from AliExpress, with stock counted from then on (see
  *Stock* in `docs/architecture.md`).

Some items vary by a property that is not the shopper's to choose, such as where they
ship from or to. On the product's *Options & variants* tab the merchant hides that
option at the value the shop buys (*Shoppers see Ships From*: *No, always France*). Only
the combinations taking that value are then on sale, shown without the option, and an
order is placed for the combination the shopper's choices resolve to, so it names that
value. The other combinations stay linked and keep being read, so the merchant can
change the value later.

The merchant checks the draft and publishes it. The same item can be imported once per
shop; one whose import failed, or that was unlinked, can be imported again. So can one
whose product was archived: archiving unlinks it (a read that finds the product
archived or gone does too), and an import asked for anyway unlinks it first.

## Keeping in step

A read of AliExpress is one call per product. Dropshippers have no call that reads
several products, and no push when one changes (checked in the open platform's API
list), so what keeps a catalogue in step is reads, and the app's calls are limited
while catalogues are not.

Each node goes through the imported products due, its own share, and asks for each to
be read again: a `SyncRequested` on the item's stream, which says who asked. A product
is due when it was last tried longer ago than `[aliexpress] sync_every_secs` (a day).
A read that failed, or a shop that cannot read for now (suspended, or no account
connected), waits for the next period like one that worked, so it takes no turn from
the others.

On a timer of its own (`sync_check_secs`, an hour by default), a node renews every
connected company's access a day before it lapses, whether or not its shops have
anything imported: the platform keeps which companies are connected and until when
(`tenancy_aliexpress_accounts`), and one node per token renews it (a
`timada-aliexpress/TokenRefresh` claim, keyed by company and token). An access
AliExpress no longer renews means an owner connects again.

Each imported product, and each order placed on AliExpress, belongs to one of the
nodes up, by a hash of its stream's id over the nodes heard from in the last 30
seconds, so one node sweeps it and one node answers a request for it, and the app's
quota is spent once. A node that stops is left out once it has been silent that long:
what it held and had not read yet is shared out to the others, and the sweep finds it
still due. Two nodes that disagree for a moment read one twice or leave it for later,
neither of which harms it.

*Sync now*, on the AliExpress page or on the product page, asks the same way, naming
the member, and waits up to ten seconds so the page can say what the read found.
Past that, the page says the read is on its way. A read does four things:

- each variant's stock becomes what AliExpress has;
- with a rule (the product's own, else the shop's), each price follows the cost;
- the product's delivery time becomes the days the service an order would ship with
  takes to the shop's country (`min_delivery_days` and `max_delivery_days` of
  `aliexpress.ds.freight.query`, the call that already prices the delivery, so it
  costs nothing more). AliExpress's days always win: the product page shows them
  without a form. No days given clears the product's, and the shop's default (the
  Shipping page) stands in. Shoppers in other countries see the same days;
- an item AliExpress no longer sells gets stock 0 everywhere and is taken off sale.
  It stays a draft until the merchant publishes it again.

A combination AliExpress dropped goes to 0. A read that fails is shown on the link;
the product keeps what it had. Unlinking stops the reads and leaves the product as it
is.

### The day's calls

`[aliexpress] calls_per_day` (12,000 by default) is what the app may spend in a day,
over every shop and node. Each node holds its part, and spends it evenly over the day:
at 12,000 alone, one call every 7.2 seconds, never a burst. A node that idled keeps a
minute of calls at most.

Every call this node makes counts: a read for a sale, a member or a cart, an import, an
order placed or read again, a renewal. Those never wait, since a sale cannot. The sweep
spends what they leave, so a busy day reads fewer of the other products, not more calls
in all. A burst of sales costs the sweep at most a day's calls. The budget is per node,
each counting its own calls, so it holds as long as the nodes are about as busy.

An open order costs a call per AliExpress order for each push, and for each day it went
without one: 500 open orders with no push at all spend about 500 calls a day.

The sweep spends the calls by priority:

1. What shoppers wanted in the last 30 days: put in a cart, or bought.
2. What is on sale.
3. The rest: drafts, and what was taken off sale.

Within each of those the shops take turns, each its oldest first, so one huge catalogue
cannot starve the others. When the budget does not cover a period, the last rank waits:
a draft may be stale when a merchant publishes it, and the read at payment protects the
sale. Pressing *Sync now* brings it up to date.

`calls_per_day = 0` leaves only what sales and members ask for.

### What a read writes

A product read often must not grow its stream with every read, so a read writes to
the item's stream (`Synced`, `SyncFailed`) only when it has something to say: a cost,
a combination or whether AliExpress sells it changed, a failure for a new reason, or a
problem that is over. A read a member asked for always records how it went, since they
wait for it. Otherwise only this node's read model notes the time (`last_synced_at`
for a read that worked, `last_tried_at` for any read asked for or made), which other
nodes learn from the next request or record. What AliExpress has left goes to the
variant's stock in the catalog and nowhere else: it moves with every sale, and the
stock streams are made for that.

### Before a payment

A shopper must not be charged at a cost the shop's price no longer covers, and goods
must not be sold after AliExpress ran out. So when a shopper asks to pay, the payments
subscription first reads every imported product in the cart, at once, and only then
prices it. The checks that were already there do the refusing:

- The rule moved the price, so the total is not the one the shopper saw:
  `price_changed`. The checkout shows the new total, and the second request finds the
  product read a moment ago.
- AliExpress ran out: `out_of_stock`, before any payment is opened, so no refund is
  owed later (`PaymentReturned` remains for a hold that lapsed during the payment).
- AliExpress stopped selling it: the product is off sale.

A product read in the last minute (`[aliexpress] read_fresh_secs`) is not read again,
so a popular product costs one call a minute per node however many shoppers pay.

AliExpress unreadable does not stop a sale. A failed read logs and the payment goes by
the last read: an unknown price is not a price that moved. Payments of every shop are
answered one at a time, so the wait is short (three seconds), and when AliExpress does
not answer in that time, payments on that node go by the last reads for a minute
without asking. The reads already under way finish on their own, and the next payment
finds them done.

A product priced by hand has no rule to move its price: the read updates its stock and
its cost, and a cost that rises above the price still sells. That is the merchant's
choice.

Each node answers a request to pay, so on a cluster each reads the cart unless its
last read is under a minute old.

### When a shopper adds to a cart

When a shopper puts an imported product in a cart, the node it belongs to asks for a
read (`SyncRequested`), unless the product was tried in the last hour. The read lands
well before the pay click, so what the checkout shows is usually already what
AliExpress asks and the read before the payment finds nothing to refuse. The hour keeps
the stream of a product selling around the clock from growing: a stream's versions run
out at 65,535.

### New combinations

A read also notes the priced combinations AliExpress sells that the product does not
(`CombinationsFound`, written with the read's `Synced`). The product page lists them
under *New on AliExpress*, and the AliExpress page counts them. Nothing changes in the
catalog until the merchant acts:

- *Add as variant* (or *Add all*) adds the values its options lack, its picture, a
  price by the rule (the cost without one) and AliExpress's stock, then records the
  link (`CombinationsAdded`). The next reads keep it in step like the others. If an
  add is not recorded, for example because a read came in between, adding the
  combination again links the variant it already made.
- *Ignore* stops offering it (`CombinationIgnored`); *Offer again* undoes that.
- A combination that does not fit is listed with the reason. That is one that varies
  by an option the product lacks, or one of a product that does not vary. The catalog
  takes no new option once variants exist, so the merchant imports the item again.
- A combination taking another value of a hidden option is listed the same way: it would
  not be on sale. Showing the option again makes it fit.

## Orders

An order whose items were imported from AliExpress can be bought there for the
shopper: its AliExpress items go into one `aliexpress.ds.order.create` call, shipped
to the order's address. AliExpress answers with one order per seller. The shopper's
money is only held on their card until then: once AliExpress took the order, the
money is taken and the invoice issued (for the whole order, its own goods too in a
mixed one). The order page has an *AliExpress* card that shows where this stands.
Two settings on the AliExpress page (`AliexpressForwardingSet`) decide how it works:

- **Place paid orders on AliExpress automatically.** Without it, the merchant presses
  *Place on AliExpress* on the order page.
- **Ask AliExpress to pay them** (`try_to_pay`). AliExpress charges the account's saved
  payment method. Without it, or when AliExpress could not charge it, the merchant
  pays in AliExpress.

What is bought:

- Each order line is the combination its variant (or the product that does not vary)
  was linked to. Lines of one combination are merged.
- A line of the shop's own, or a variant added by hand, stays with the merchant, and
  the card says so.
- How AliExpress orders the combination (`sku_attr`) is read again from the product
  when the order is placed, and so is its stock.
- Each item ships with **a delivery AliExpress runs itself** (a Cainiao service):
  free before paid, tracked first, the cheapest paid one otherwise. Without one, the
  cheapest **tracked service of the seller's**, as an item in a local warehouse often
  has only that (`SELLER_SHIPPING_DE`). When nothing offered can be followed, nothing
  is placed and the card says what was offered.
- The address goes as given, with the phone split into its calling code and national
  number: the shopper's, or the shop's phone from the AliExpress page when they gave
  none. AliExpress also wants a `province`, from its own list where the country has one: a
  French address names the postal code's department (`Rhone`, `Paris`…), and Belgium,
  Spain, Canada and Italy work the same way. Other countries with a list send its `Other`,
  and those without one send the city. A French overseas department goes as the
  country AliExpress lists it under (`MQ`, `GP`, `GF`, `RE`, `YT`), for the delivery
  as well. The town then finds AliExpress's city, and the province where the postal
  code named none, in the country's list; a town the list files under a city stays in
  the address's second line. [aliexpress-provinces.md](aliexpress-provinces.md) has
  every country.

Before anything is bought, Stripe is asked whether it holds the payment the order
names, opened for this shop's cart: an order written into the shop's store by other
means than the checkout (a `store:write` key's) is refused with *it was not placed
through the shop's checkout*, a hold that went back meanwhile is refused too, and
Stripe not answering fails the forward to try again.

A failure is shown on the card with *Place again*. One node takes each order on
(`Started`, like an import), and right before asking AliExpress writes `Asking` on the
forward at the version it took it on at: an attempt overtaken in between (asked for
again, given up as stalled) asks nothing. What AliExpress took is recorded at once,
tried again for a few seconds while the store refuses writes; past that it is kept on
the node that placed it (`aliexpress_unrecorded`, one row per placement, in the
read-model database, which a copy from a peer leaves alone) and recorded by that node's
sweep once the store takes writes again, however long that is. The stall sweep may give
up a forward whose node is only slow (it does not know which node asks); what that node
records later still places it. An attempt whose orders land on a forward an
overlapping attempt placed meanwhile records them beside it (`AlsoPlaced`), and the
card asks the merchant to cancel them on AliExpress. If AliExpress did not answer, or a
node stopped, the order may be on AliExpress anyway: a forward taken on and not heard
of for half an hour fails saying so (the sweep of the node it belongs to), and the card
offers *Place again* for a forward taken on that long. It is then never placed again on
its own: the merchant checks the account's orders on AliExpress first.

Right before AliExpress is asked, the forwarder writes `Order::SupplierOrderStarted` on
the shop's order, at the version it read, on every try. A cancellation (the shopper's,
the merchant's) landing first refuses it, so nothing is bought for a cancelled order;
one landing after finds it, and then knows AliExpress may have the order: the shopper
can no longer cancel, and the merchant ticks that they cancel it on AliExpress too
(there is no API to). A sure refusal (`SupplierOrderRefused`) lets the shopper cancel
again; an attempt that may have placed the order (`SupplierOrderUncertain`) does not,
and keeps the fence whatever a later attempt is told, until the merchant, having looked
on AliExpress, presses *Not on AliExpress* on the card (`SupplierOrderRuledOut`). An
attempt begun while the one before never said how it went counts that one uncertain
too. The card offers *Not on AliExpress* only once the forward failed, and half an hour
after the order became uncertain, as AliExpress's list of orders may take that long to
show one it took. The order is told how an attempt went before the forward is, tried
again while the store refuses; an attempt the order never hears of leaves the forward
taken on, for the stall sweep. An order whose card hold lapses within twelve hours is
not placed: the merchant takes the payment first. A hold that lapses anyway while
AliExpress may have the order cancels nothing: the order records the hold gone and
stays open, for the merchant to settle with the shopper and cancel. Nothing is held, so
that cancellation asks Stripe nothing, and it settles even when the shop took its
Stripe keys away. Once every parcel of such an order is known, its forward closes and
is no longer read, so a later change on AliExpress (the delivery, a refund to the
account) does not show on its card.

AliExpress pushes each status change of an order to `/aliexpress/webhook` (message type
53, the body signed with the app's key and secret in `Authorization`). The push is
recorded in the platform's stream of that AliExpress order (`timada-aliexpress/AeOrder`)
and answered, within the half second AliExpress allows; one the store did not take is
answered with an error, and AliExpress sends it again. The subscription
`timada-dashboard/aliexpress-pushes` then reads the order again
(`aliexpress.trade.ds.order.get`) on the node the order belongs to. A push may come
twice: the read records against the version it started from, so a second one changes
nothing. Each forward records its AliExpress orders in those streams as AliExpress
answers (`Claimed`), so a push finds its shop without a read model; one that came first
(AliExpress pushing as it answers) waits there, and is read once the record lands. The
reads run beside the subscription, a few at a time, so a burst of pushes while
AliExpress is slow holds none of them up. A number already recorded for another
forward is refused and logged as an error.

### Trying the pushes from a development machine

AliExpress cannot reach a laptop, so `make aliexpress.tunnel` opens an ngrok tunnel
to the dev server (`make dev`, port 3000), as `make stripe.listen` does for Stripe.
The dev shell has ngrok; `ngrok config add-authtoken <token>` once, with the token of
a free ngrok account. Claim the account's free static domain in ngrok's dashboard and
run the tunnel with `NGROK_URL=https://<that domain>`, so the address outlives a
restart; then register `https://<that domain>/aliexpress/webhook` as the app's
message push callback in the App Console, with `DROPSHIPPER_ORDER_STATUS_UPDATE`
ticked. ngrok's inspector at http://127.0.0.1:4040 shows every push with its answer
and replays one. A push answered 401 was signed with another key or secret than
`[aliexpress]` in `config/dev.local.toml` holds: the pushes come from the app whose
callback it is. AliExpress documents an OV or EV certificate for the callback, and
ngrok's domains carry a DV one; whether AliExpress holds to that is to be seen.

To connect a buyer account through the same tunnel (AliExpress sends the owner back
to `server.url`), start the dev server with `TIMADA__SERVER__URL=https://<that domain>`
and register `/aliexpress/callback` under it as the app's callback.

To check the route with no AliExpress at all, sign a push by hand with the app's key
and secret (HMAC-SHA256 over the key followed by the body, keyed by the secret) and
post it:

```sh
BODY='{"data":{"orderId":1105933950463697,"orderStatus":"OrderShipped"},"message_type":53}'
SIGNATURE=$(printf '%s%s' "$APP_KEY" "$BODY" \
  | openssl dgst -sha256 -mac HMAC -macopt key:"$APP_SECRET" | sed 's/^.* //')
curl -i -X POST https://<that domain>/aliexpress/webhook \
  -H "content-type: application/json" -H "authorization: $SIGNATURE" -d "$BODY"
```

It answers 200 and the push waits in the order's stream (`timada-aliexpress/AeOrder`)
until a forward records that order; the dashboard logs it as *the status of an order
no forward recorded yet*.

A push can still be lost, past its twelve retries half an hour apart, and a node
catching up leaves the reads of pushes older than ten minutes. So the node an order
belongs to also reads it once it has gone unread for
`[aliexpress] order_read_every_secs` (a day), the shops taking
turns, oldest read first. Without the callback registered, tracking arrives that late:
lower the setting.

Once each order has its parcel number, every line of the shop's order was bought
there and its money was taken, the order is fulfilled with those numbers as its
tracking.

AliExpress may close an order without shipping it: its push says `OrderClosed`, which
stands even when the order cannot be read, and a read showing `FINISH` with no parcel
ever known says the same (that read-side rule awaits the real-app test, #253). Once
every AliExpress order of a shop's order AliExpress alone was to fill is closed, the
shop's order is cancelled: its hold is given back, or the money it took refunded, by
the dashboard's payments (`timada-dashboard/order-payments`). A mixed order, or one
only partly closed, is shown on the card for the merchant to settle. A
mixed order only shows them, for the merchant to fulfil. An order fulfilled by hand,
cancelled, or placed more than 60 days ago is no longer read. The request and
response shapes follow ae_sdk, like the rest: the real-app smoke test (#253) covers
these calls too.

The order and the forward are two streams, so this is three writes: what was read, the
order's `Fulfilled`, then the forward's `Done`. A node that stops in between loses
nothing. Only that leaves an order open with every parcel known, so such an order is
due on every sweep (`sync_check_secs`) rather than once a day. The next read fulfils
it from the reads recorded, or closes the forward with the tracking the order was
fulfilled with, whether AliExpress answers or not.

## Where things live

- **Platform store (`timada-tenancy`).** On the company, the sealed tokens of its
  buyer account: `Company::AliexpressAuthorized`, `Company::AliexpressDisconnected`.
  On the tenant, the market, the shop's rule and how orders go:
  `AliexpressConfigured`, `AliexpressPricingSet`, `AliexpressRemoved`,
  `AliexpressForwardingSet`. The tokens
  are sealed under `secrets.key` with the purpose `aliexpress`; the app's key and
  secret are host configuration.
- **Tenant store (`timada-aliexpress/Product`).** One stream per item: the import's
  steps, the catalog product, each sku's variant and cost, the reads asked for, those
  that found something, and the product's own rule. The `aliexpress_products` read
  model lists them.
- **Tenant store (`timada-aliexpress/Forward`).** One stream per order placed on
  AliExpress: the request, the claim, what was bought and the AliExpress orders, each
  read that changed something, and the end. The `aliexpress_forwards` read model
  lists the ones the sweep still reads, and those left being placed.
- **Platform store (`timada-aliexpress/AeOrder`).** One stream per order AliExpress
  holds for a company's account, by its AliExpress number: the forward that placed it
  (`Claimed`, with its tenant) and every push for it (`Pushed`). A push names nothing
  else, so this is where it finds its shop.
- **Calls.** These are signed like moh3a/ae_sdk: HMAC-SHA256 over the API path (for
  path calls), then every parameter sorted by name as `name` + `value`, in upper-case
  hex. Dotted methods go to `/sync` and path methods to `/rest<path>`. The response
  field names follow ae_sdk's types, and every field is read leniently.

Pictures are fetched without following redirects, and only from public hosts unless
`mail.allow_private_hosts` is set.
