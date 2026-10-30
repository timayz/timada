# AliExpress

A shop imports products from AliExpress's dropshipping catalogue through the
**platform's** AliExpress app, which acts for the AliExpress buyer account of the
company the shop sells as. The dashboard keeps each imported product in step with
AliExpress: its stock, its price when a rule prices it, and whether AliExpress still
sells it.

## Setting it up (operator, once)

1. On AliExpress's open platform, create an app with access to the dropshipping API.
   AliExpress has to approve that access before product calls work.
2. Register `/aliexpress/callback` under `server.url` as the app's callback.
3. Put the app's key and secret in `[aliexpress]` (`app_key`, `app_secret`), with
   `secrets.key` set so the tokens can be sealed. Without both, nobody can connect.

AliExpress counts every call against the one app, whichever company it acts for: the
sweep paces its reads, and the intervals may need to grow with the number of shops.

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
3. Optionally set a **price rule**: multiply the cost, add an amount, and round to the
   cent, up to the next .99, or up to a whole amount. A rule never goes below cost.
   Without one, the import uses the cost as the starting price and later reads leave
   prices alone.

   Cheap items usually bear a higher rate than dear ones, so the rule can go by
   **cost band**: each row prices costs up to its amount (inclusive) that no lower
   band took, and the last row prices any higher cost — say ×2.5 up to 30.00, ×2 up to
   100.00, ×1.5 above. Up to ten bands; each has its own multiplier, amount added and
   rounding. A blank row is dropped, and two bands ending at the same amount are
   refused. A product's own rule (on its page) is one rate for every cost.

*Stop importing* forgets the shop's settings; the imported products stay as they are,
and so does the company's account.

## Importing

Paste a product's number or address and press *Import*. The dashboard fetches the
product in the background, on the first node to take it on. It then fills a **draft**:

- the name, the description as plain text, the facts, and up to twenty pictures,
  including each colour's own;
- one option per AliExpress property and one variant per priced combination, the
  AliExpress sku id as its code;
- each variant's stock from AliExpress, with stock counted from then on (see
  *Stock* in `docs/architecture.md`).

The merchant checks the draft and publishes it. The same item can be imported once per
shop; one whose import failed, or that was unlinked, can be imported again.

## Keeping in step

Every node sweeps the imported products on a timer (`[aliexpress] sync_check_secs`,
an hour by default) and reads again each one last read longer ago than
`sync_every_secs` (six hours). Reads are paced and go one shop after another; a read
another node recorded in the meantime wins. Before reading, the same sweep renews
every connected company's access a day before it lapses, whether or not its shops
have anything imported: the platform keeps which companies are connected and until
when (`tenancy_aliexpress_accounts`), and one node per token renews it (a
`timada-aliexpress/TokenRefresh` claim, keyed by company and token). An access
AliExpress no longer renews means an owner connects again.

*Sync now*, on the AliExpress page or on the product page, reads the item at once.
A read does three things:

- each variant's stock becomes what AliExpress has;
- with a rule (the product's own, else the shop's), each price follows the cost;
- an item AliExpress no longer sells gets stock 0 everywhere and is taken off sale.
  It stays a draft until the merchant publishes it again.

A combination AliExpress dropped goes to 0. A read that fails is shown on the link;
the product keeps what it had. Unlinking stops the reads and leaves the product as it
is.

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

## Orders

A paid order whose items were imported from AliExpress can be bought there for the
shopper: its AliExpress items go into one `aliexpress.ds.order.create` call, shipped
to the order's address. AliExpress answers with one order per seller. The order page
has an *AliExpress* card that shows where this stands. Two settings on the AliExpress
page (`AliexpressForwardingSet`) decide how it works:

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
- Each item ships with **free delivery by AliExpress itself** (a Cainiao service),
  tracked first. When AliExpress offers none to that country, nothing is placed and the
  card says why. The address goes as given, with the phone split into its calling
  code and national number. There is no province field.

A failure is shown on the card with *Place again*. One node takes each order on
(`Started`, like an import). If AliExpress did not answer, or a node stopped before
recording the outcome, the order may be on AliExpress anyway. It is then never placed
again on its own: the merchant checks the account's orders on AliExpress first.

Until each AliExpress order has a parcel number, the sweep reads it again on every
pass (`aliexpress.trade.ds.order.get`). Once each does, and every line of the shop's
order was bought there, the order is fulfilled with those numbers as its tracking. A
mixed order only shows them, for the merchant to fulfil. An order fulfilled by hand,
cancelled, or placed more than 60 days ago is no longer read. The request and
response shapes follow ae_sdk, like the rest: the real-app smoke test (#253) covers
these calls too.

## Where things live

- **Platform store (`timada-tenancy`).** On the company, the sealed tokens of its
  buyer account: `Company::AliexpressAuthorized`, `Company::AliexpressDisconnected`.
  On the tenant, the market, the shop's rule and how orders go:
  `AliexpressConfigured`, `AliexpressRemoved`,
  `AliexpressForwardingSet`. The tokens
  are sealed under `secrets.key` with the purpose `aliexpress`; the app's key and
  secret are host configuration.
- **Tenant store (`timada-aliexpress/Product`).** One stream per item: the import's
  steps, the catalog product, each sku's variant, cost and last stock, the reads and
  the product's own rule. The `aliexpress_products` read model lists them.
- **Tenant store (`timada-aliexpress/Forward`).** One stream per order placed on
  AliExpress: the request, the claim, what was bought and the AliExpress orders, each
  read that changed something, and the end. The `aliexpress_forwards` read model
  lists the ones the sweep still reads.
- **Calls.** These are signed like moh3a/ae_sdk: HMAC-SHA256 over the API path (for
  path calls), then every parameter sorted by name as `name` + `value`, in upper-case
  hex. Dotted methods go to `/sync` and path methods to `/rest<path>`. The response
  field names follow ae_sdk's types, and every field is read leniently.

Pictures are fetched without following redirects, and only from public hosts unless
`mail.allow_private_hosts` is set.
