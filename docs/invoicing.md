# Invoicing

Every paid order gets an invoice and every refund a credit note (*avoir*), issued by
the merchant to the shopper under French rules. Nobody has to press a button: the
dashboard issues them a moment after the payment or the refund.

## Before the first sale

On `/t/<slug>/invoicing` (permission `tenant:manage`) the merchant records who it
is in law:

- legal name and form, and the share capital if the form has one;
- SIRET, which must pass its Luhn check;
- the RCS city, if the company is registered;
- the registered address and a contact e-mail;
- VAT, one of two ways:
  - a VAT number, which must embed the SIREN under its key, and whether the merchant
    is registered for the EU One-Stop Shop;
  - nothing, for the exemption for small businesses. The invoices then carry
    *TVA non applicable, art. 293 B du CGI*.

Until this is set, checkout refuses to take a payment (`invoicing_not_configured`):
no sale goes through without an invoice that can name the seller. A VAT-registered
merchant says on the product page what kind of goods each product is: standard,
books, food, medicines or press. In France that is 20, 5.5, 5.5, 10 and 2.1 %. Catalog
prices always include VAT.

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
- **VAT and totals.** VAT per rate: base, tax and total. Then the totals excluding
  VAT, of VAT, and including VAT.
- **Payment.** *Facture acquittée : payée par carte bancaire le …*

VAT is worked out once per rate on the sum of its lines, never per line, so the
rounding never drifts. A credit note corrects one invoice: it names it, and spreads
the refunded amount over the invoice's rates in proportion, to the cent.

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
  excluding VAT, all countries together (art. 258 A du CGI).
  - Each invoice carries the year's running total.
  - The sale that passes the threshold is already taxed at destination, and so is
    everything after it, until the end of the next year.
  - The total is counted inside the same append that takes the number, so two
    invoices issued at once never both count against a stale total.
  - Only sales in euros count. An EU distance sale in another currency is taxed at
    destination, which is the safe side.
- **One-Stop Shop.** The merchant ticks "One-Stop Shop" on the invoicing page to
  apply destination VAT from the first euro.
  - The invoicing page shows the year's total.
  - `/t/<slug>/invoices/oss?quarter=2026-Q4` sums each country's VAT and rate for
    the quarterly OSS return.
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
  **Have an accountant review the table**: what counts as a reduced-rate good
  differs from country to country and changes.

## Numbering

There is one series per kind and calendar year (UTC): `F2026-000001`, `F2026-000002`
and so on for invoices, `A2026-000001` for credit notes. Within a series the numbers
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
- Shipping costs and discounts, which orders do not have yet.
