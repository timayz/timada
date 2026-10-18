# AliExpress

A shop imports products from AliExpress's dropshipping catalogue through the merchant's
**own** AliExpress app. The dashboard keeps each imported product in step with
AliExpress: its stock, its price when a rule prices it, and whether AliExpress still
sells it.

## Setting it up (merchant)

1. On AliExpress's open platform, create an app with access to the dropshipping API.
   AliExpress has to approve that access before product calls work.
2. Register the callback address the dashboard shows on `/t/{slug}/aliexpress` as the
   app's callback. Each shop has its own address, because each merchant has their own
   app.
3. On that page, enter the app key and secret and choose what the shop buys in:
   - the country goods ship to;
   - the currency, one the shop sells in;
   - the language products are worded in.
4. Optionally set a **price rule**: multiply the cost, add an amount, and round to the
   cent, up to the next .99, or up to a whole amount. A rule never goes below cost.
   Without one, the import uses the cost as the starting price and later reads leave
   prices alone.
5. Press *Connect the AliExpress account*. AliExpress asks the account to say yes,
   then sends the merchant back. The tokens are kept sealed.

The member needs the `aliexpress:manage` permission.

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
another node recorded in the meantime wins. The same sweep renews an account's access
a day before it lapses, on one node per token (a `timada-aliexpress/TokenRefresh`
claim). An access AliExpress no longer renews means the merchant connects again.

*Sync now*, on the AliExpress page or on the product page, reads the item at once.
A read does three things:

- each variant's stock becomes what AliExpress has;
- with a rule (the product's own, else the shop's), each price follows the cost;
- an item AliExpress no longer sells gets stock 0 everywhere and is taken off sale.
  It stays a draft until the merchant publishes it again.

A combination AliExpress dropped goes to 0, and new combinations are not added. A read
that fails is shown on the link; the product keeps what it had. Unlinking stops the
reads and leaves the product as it is.

## Where things live

- **Platform store (`timada-tenancy`).** The app key, the sealed secret, the market,
  the shop's rule and the sealed tokens: `AliexpressConfigured`,
  `AliexpressAuthorized`, `AliexpressRemoved`. They are sealed under `secrets.key`
  with the purpose `aliexpress`.
- **Tenant store (`timada-aliexpress/Product`).** One stream per item: the import's
  steps, the catalog product, each sku's variant, cost and last stock, the reads and
  the product's own rule. The `aliexpress_products` read model lists them.
- **Calls.** These are signed like moh3a/ae_sdk: HMAC-SHA256 over the API path (for
  path calls), then every parameter sorted by name as `name` + `value`, in upper-case
  hex. Dotted methods go to `/sync` and path methods to `/rest<path>`. The response
  field names follow ae_sdk's types, and every field is read leniently.

Pictures are fetched without following redirects, and only from public hosts unless
`mail.allow_private_hosts` is set.
