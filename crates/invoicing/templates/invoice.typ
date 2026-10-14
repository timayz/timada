// An invoice or a credit note of a French merchant, layout version 1.
// Every value comes in as text already written the French way
// (`timada_invoicing::pdf`): the template only places it. Do not change
// what this file draws: documents issued with version 1 are drawn again
// from it for as long as they are kept. A new layout is a new file.

#import sys: inputs
#let d = inputs

#set document(
  title: d.title,
  author: d.seller_name,
  date: datetime(year: d.issued_year, month: d.issued_month, day: d.issued_day),
)
#set page(paper: "a4", margin: (x: 2cm, top: 2cm, bottom: 2.5cm), footer: context [
  #set text(8pt, fill: luma(90))
  #d.seller_name — #d.seller_form_capital — SIRET #d.seller_siret#if d.seller_rcs != "" [ — #d.seller_rcs]
  #h(1fr) #counter(page).display("1 / 1", both: true)
])
#set text(font: "Libertinus Serif", size: 10pt, lang: "fr")
#set par(spacing: 0.6em)

#let muted(body) = text(fill: luma(90), body)
#let address(lines) = lines.join(linebreak())

#grid(
  columns: (1fr, auto),
  gutter: 1em,
  [
    #text(14pt, weight: "bold", d.seller_name) \
    #address(d.seller_address) \
    #muted[#d.seller_email] \
    #muted[SIRET : #d.seller_siret] \
    #if d.seller_vat != "" [#muted[N° TVA intracommunautaire : #d.seller_vat]]
  ],
  align(right)[
    #text(20pt, weight: "bold", d.kind_title) \
    #text(12pt)[N° #d.label] \
    Date d'émission : #d.issued_on \
    Commande : #d.order_reference \
    #if d.sale_on != "" [Date de la vente : #d.sale_on \ ]
    #if d.corrects != "" [Facture d'origine : #d.corrects]
  ],
)

#v(1.5em)
#grid(
  columns: (1fr, 1fr),
  gutter: 2em,
  [
    #muted[Client] \
    #strong(d.buyer_address.first()) \
    #address(d.buyer_address.slice(1)) \
    #muted[#d.buyer_email]
    #if d.buyer_vat != "" [ \ N° TVA client : #d.buyer_vat]
  ],
  if d.delivery_address.len() > 0 [
    #muted[Adresse de livraison] \
    #strong(d.delivery_address.first()) \
    #address(d.delivery_address.slice(1))
  ],
)

#v(1em)
#muted[Catégorie de l'opération : #d.category]
#v(0.5em)

#table(
  columns: (1fr, auto, auto, auto, auto),
  align: (left, right, right, right, right),
  stroke: (x, y) => if y == 0 { (bottom: 0.6pt) } else { (bottom: 0.3pt + luma(200)) },
  inset: (x: 4pt, y: 5pt),
  table.header[*Désignation*][*Qté*][*P.U. HT*][*TVA*][*Total TTC*],
  ..d.lines.map(line => (
    [#line.name#if line.detail != "" [ \ #muted(text(8.5pt, line.detail))]],
    line.quantity,
    line.unit_excl,
    line.rate,
    line.total,
  )).flatten(),
)

#v(0.5em)
#grid(
  columns: (1fr, auto),
  gutter: 2em,
  if d.vat.len() > 0 [
    #table(
      columns: (auto, auto, auto, auto),
      align: right,
      stroke: none,
      inset: (x: 4pt, y: 3pt),
      [*Taux*], [*Base HT*], [*TVA*], [*TTC*],
      ..d.vat.map(row => (row.rate, row.base, row.vat, row.total)).flatten(),
    )
  ],
  table(
    columns: (auto, auto),
    align: (left, right),
    stroke: none,
    inset: (x: 4pt, y: 3pt),
    [Total HT], d.total_excl,
    [Total TVA], d.total_vat,
    table.hline(stroke: 0.6pt),
    [*#d.total_title*], [*#d.total*],
  ),
)

#v(1em)
#if d.vat_mention != "" [#strong(d.vat_mention) \ ]
#d.settlement

