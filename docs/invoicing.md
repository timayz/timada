# Invoicing

Every paid order gets an invoice and every refund a credit note (*avoir*), issued by
the merchant to the shopper under French rules. Nobody has to press a button: the
dashboard issues them a moment after the payment or the refund.

## Before the first sale

Invoices name a **company**, the legal entity that sells. One company can sell
through several tenants (shops); its tenants' invoices then count together towards
its VAT obligations.

- **Registering.** A signed-in user registers a company on `/companies/new`, or
  through "Register a new company" on a tenant's `/t/<slug>/invoicing` page. The
  user becomes its first owner. A company holds:
  - its legal name and form, and its share capital;
  - its SIRET, which must pass its Luhn check;
  - its RCS city, registered address and contact e-mail;
  - its VAT, one of two ways:
    - a VAT number, which must embed the SIREN under its key, and whether the
      company is registered for the EU One-Stop Shop;
    - nothing, for the exemption for small businesses. The invoices then carry
      *TVA non applicable, art. 293 B du CGI*.
- **A SIREN is unique on the platform.** A second registration of the same SIREN is
  refused (`siren_taken`). A platform administrator releases a SIREN someone
  registered without owning it, on `/admin/companies`.
- **Owners.** A company's owners, and nobody else, see and edit its page
  `/companies/<id>`: its identity, its owners (added by the e-mail of a registered
  user), its tenants, its EU sales of the year and its One-Stop Shop return.
- **Linking a tenant.** On the tenant's `/t/<slug>/invoicing`, which needs
  `tenant:manage`, the member links the tenant to one of the companies they own.
  - The tenant takes a **series code**:
    - the company's first tenant usually keeps the plain series (`F2026-000001`);
    - every other one needs a code (`PRO` numbers `PRO-F2026-000001`).
  - A code stays its tenant's for good, even after unlinking, so no two tenants of
    a company ever number the same way.
- **No company, no sale.** Until the tenant is linked to a company with an
  identity, checkout refuses to take a payment (`invoicing_not_configured`).
- **Kind of goods.** A VAT-registered company's tenants say on each product page
  what kind of goods it is: standard, books, food, medicines or press. In France
  that is 20, 5.5, 5.5, 10 and 2.1 %. Catalog prices always include VAT.

## What a document says

These are the mentions of CGI art. 242 nonies A and Code de commerce L441-9, plus the
cheap ones of the 2026 e-invoicing reform:

- **Number and dates.** The number, the issue date, and the date of the sale (when
  it was paid).
- **The seller.** Legal name, form and capital, registered address, SIRET (and RCS),
  VAT number.
- **The buyer.** Name and address, e-mail, the delivery address, and the category of
  the operation (*livraison de biens*).
- **The lines.** Quantity, unit price excluding VAT, VAT rate and amount including
  VAT.
- **The discount.** When the order was paid with a discount code, a negative
  *Remise CODE* row under the lines, before and including VAT.
- **VAT and totals.** VAT per rate: base, tax and total. Then the totals excluding
  VAT, of VAT, and including VAT.
- **Payment.** *Facture acquittée : payée par carte bancaire le …*

VAT is worked out once per rate on the sum of its lines, never per line, so the
rounding never drifts. A credit note corrects one invoice: it names it, and spreads
the refunded amount over the invoice's rates in proportion, to the cent.

A discount code takes its amount off the whole order. The lines keep their full
price; the discount is spread over them in proportion to the cent (largest
remainders), and the VAT, the totals and the EU distance sales are those of what was
really paid. A credit note against a discounted invoice therefore credits VAT at
the rates that were paid.

## Selling abroad

The shopper always pays the catalog price: VAT is included and never added on top.
What changes with the destination is how that price splits into base and tax. The
invoice decides it when it is issued, seconds after the payment.

| Where the goods go | Who buys | Regime | VAT on the invoice |
|---|---|---|---|
| France (and Monaco) | anyone | `domestic` | French rate of the product's kind |
| Another EU country | a business whose VAT number VIES confirms | `reverse_charge` | none: "Autoliquidation", art. 262 ter I du CGI, both VAT numbers |
| Another EU country | a consumer, under the yearly threshold | `distance_origin` | French rate |
| Another EU country | a consumer, past the threshold or with OSS ticked | `oss` | that country's rate for the product's kind |
| Outside the EU VAT area (US, CA, AE, GB, French overseas departments, Canary Islands…) | anyone | `export` | none: art. 262 I du CGI (the merchant keeps the VAT share) |
| anywhere | the merchant has no VAT number | `franchise` | none: art. 293 B du CGI (an export says so instead) |

- **The threshold** is €10,000 a year of sales to consumers in other EU countries,
  excluding VAT, all countries together, and **every tenant of the company
  together** (art. 258 A du CGI).
  - It is counted in the company's ledger, in the platform store; each such
    invoice carries the running total.
  - The sale that passes the threshold is already taxed at destination, and so is
    everything after it, until the end of the next year.
  - The sale is counted, with the decision it got, before it takes its number.
    That count is an append at the version the ledger was read at, so two tenants
    selling at once never both count against a stale total. A retry finds its
    sale counted and gets the same decision back.
  - Only sales in euros count. An EU distance sale in another currency is taxed at
    destination, which is the safe side.
- **One-Stop Shop.** The merchant ticks "One-Stop Shop" on the invoicing page to
  apply destination VAT from the first euro.
  - The company's page shows the year's total.
  - `/companies/<id>/oss?quarter=2026-Q4` sums each country's VAT and rate across
    every tenant of the company, for the quarterly OSS return.
- **VIES.** A business buyer types its VAT number at checkout. The dashboard
  checks it with VIES when invoicing, naming the seller's own number, and keeps
  VIES's reference on the invoice.
  - An invalid number makes the buyer a consumer.
  - If VIES is unreachable after three tries, the invoice charges VAT (the safe
    side) and a warning is logged.
  - The `[invoicing] vies_url` setting points elsewhere (tests use a stand-in).
- **Rates.** A product's kind (standard, books, food, medicines, press) maps to a
  rate level in each EU country through `crates/invoicing/data/tax_categories.json`.
  The rate of each level comes from the `world-tax` crate, pinned exactly.
  - The table was reviewed on 2026-10-17 against the Commission's "VAT rates
    applied in the Member States" (January 2025) and the national changes of 2025
    and 2026. That review is not a professional opinion: what counts as a
    reduced-rate good differs from country to country and changes, so **have an
    accountant confirm the table** before relying on it for a new market.
  - The file's `_review` block records the date, the sources, what the review
    changed and what it could not settle. A test fails once the date is more than
    eighteen months old, so the check comes back on its own.
  - The categories are coarse on purpose. A country that taxes basic food, daily
    newspapers or prescription medicines differently from the rest of the kind
    gets the rate of the wider group; the `_review.to_confirm` list names those.

### Rates by country

The VAT a product of each kind carries when its sale is taxed in that country,
from the table and the pinned `world-tax` rates. A test rebuilds this table and
fails when it differs from the data; its message holds the replacement.

<!-- tax-categories:start -->
| Country | Standard | Books | Food | Medicines | Press |
|---|---|---|---|---|---|
| AT | 20 % | 10 % | 10 % | 10 % | 10 % |
| BE | 21 % | 6 % | 6 % | 6 % | 6 % |
| BG | 20 % | 9 % | 20 % | 20 % | 9 % |
| CY | 19 % | 5 % | 5 % | 5 % | 5 % |
| CZ | 21 % | 0 % | 12 % | 12 % | 12 % |
| DE | 19 % | 7 % | 7 % | 19 % | 7 % |
| DK | 25 % | 25 % | 25 % | 25 % | 0 % |
| EE | 24 % | 9 % | 24 % | 9 % | 9 % |
| ES | 21 % | 4 % | 10 % | 4 % | 4 % |
| FI | 25.5 % | 14 % | 14 % | 10 % | 10 % |
| FR | 20 % | 5.5 % | 5.5 % | 10 % | 2.1 % |
| GR | 24 % | 6 % | 13 % | 6 % | 6 % |
| HR | 25 % | 5 % | 5 % | 5 % | 5 % |
| HU | 27 % | 5 % | 5 % | 5 % | 5 % |
| IE | 23 % | 0 % | 0 % | 0 % | 9 % |
| IT | 22 % | 4 % | 10 % | 10 % | 4 % |
| LT | 21 % | 5 % | 21 % | 5 % | 5 % |
| LU | 17 % | 3 % | 3 % | 3 % | 3 % |
| LV | 21 % | 12 % | 21 % | 12 % | 12 % |
| MT | 18 % | 5 % | 0 % | 0 % | 5 % |
| NL | 21 % | 9 % | 9 % | 9 % | 9 % |
| PL | 23 % | 5 % | 5 % | 8 % | 8 % |
| PT | 23 % | 6 % | 6 % | 6 % | 6 % |
| RO | 21 % | 11 % | 11 % | 11 % | 11 % |
| SE | 25 % | 6 % | 12 % | 25 % | 6 % |
| SI | 22 % | 5 % | 9.5 % | 9.5 % | 5 % |
| SK | 23 % | 5 % | 5 % | 5 % | 5 % |
<!-- tax-categories:end -->

Known gaps, waiting on an accountant or on `world-tax`:

- Latvia taxes books and press at 5 % (law of 18 December 2025). The `world-tax`
  table has no 5 % level for Latvia, so they carry 12 % here.
- Lithuania abolished its 9 % band on 1 January 2026; books moved to 5 %, but
  periodicals are not named in that band, so their rate is unconfirmed.
- Belgium zero-rates daily and weekly newspapers; other periodicals are 6 %.
- Spain and Italy tax basic foodstuffs at 4 % and other food at 10 %.

## Numbering

There is one series per tenant, kind and calendar year (UTC): `F2026-000001`,
`F2026-000002` and so on for invoices, `A2026-000001` for credit notes. A tenant
with a series code gets `PRO-F2026-…` and `PRO-A2026-…`, so the numbers of a
company's tenants never collide. A credit note follows its invoice's code. Within a series the numbers
follow each other with no gap, in the order the documents were issued.

How it holds under retries and several nodes (details in
`crates/invoicing/src/command/mod.rs`):

- **Each document is written whole, once.** It goes into the stream of its series
  (a *book*), at the version the book was read at. The number and its document are
  therefore one write: a number is never taken without its document, and two
  writers never take the same one.
- **A book holds 1,000 documents.** After that the series carries on in the next book.
- **A document first reserves its series.** It does so in a small index stream of
  its own. A retry after a crash finds the document in the book instead of taking a
  second number, even across New Year.

## Keeping them

A document never changes once issued. It copies everything it says: the seller as
it was then, the buyer, the lines, the VAT. Changing the legal identity later does
not touch invoices already issued.

The PDF is not stored. It is drawn on demand from the document with Typst, in
PDF/A-2b, and the same document always draws the same bytes. The layout is
versioned (`template` in the event): a new layout is a new template file, and old
documents keep the one they were issued with. That is how they stay readable for the
ten years the law asks.

## Where to find them

- **Dashboard.**
  - The order page lists its invoice and credit notes.
  - `/t/<slug>/invoices` lists every document, newest first. It needs `orders:read`,
    and each entry downloads as PDF.
- **Storefront.**
  - `timada_storefront::documents(cx, order_id)` lists an order's documents.
  - `document_pdf(cx, order_id, document_id)` draws one. It only answers for a
    document of that order.
  - The example app links them from the order confirmation page, for the browser
    that placed the order.

## Not yet

- Mailing the invoice to the shopper.
- Factur-X (the PDF/A-3 with embedded XML that French B2B e-invoicing will need).
- A business buyer's SIREN (French B2B e-invoicing).
- Currency conversion: VAT on a non-euro invoice must also be stated in euros.
- Northern Ireland (goods follow EU VAT rules there) and other special territories
  beyond the ones listed above.
- Shipping costs, which orders do not have yet.
- A rate the `world-tax` table lacks (Latvia's 5 % on books and press) cannot be
  expressed; it needs a local override or an upstream fix.
