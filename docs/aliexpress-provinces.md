# AliExpress: the State/Province of a forwarded order

AliExpress takes no order without a `province`, and checks it against its own
list for the country. The shop's addresses carry no region (street, postal
code, city, country, phone), so the forwarder derives one:

- **France:** the postal code's department, named as AliExpress lists them
  (`Rhone`, `Paris`, `Cote-d'Or`, `Corse-du-Sud`…), `Other` for a postal code
  the list lacks. Done, `french_department` in
  `crates/aliexpress/src/forwarding.rs`.
- **French overseas departments:** the checkout files them under France,
  AliExpress lists them as countries, so `delivery_country` turns the postal
  code into one for the address and the delivery. Martinique's State/Province
  is a mandatory free text there, and takes the city.
- **Every other country:** the city, a stand-in. It passes only where
  AliExpress has no list; elsewhere the order is refused with
  `Please select a State/Province/County`.

## Collecting a country's list

In AliExpress, open the account's address book, add an address, pick the
country, open the "State/Province" dropdown and copy its markup (the `<ul
role="listbox">`), as was done for France. Paste it in the issue or PR that
maps the country.

## Countries the storefront ships to

One entry per country of the checkout's list, the likeliest to forward
first. "Postal code →" says how well a postal code alone names the
subdivision AliExpress is likely to list: **exact** can be mapped server-side
like France, **approx** needs care at the borders, **none** needs the
shopper to pick it at checkout. Tick the country once both boxes under it
are.

- [x] **France (FR)** — AliExpress list: 94 metropolitan departments, unaccented, + `Other` · postal code → exact · #359
  - [x] dropdown pasted
  - [x] forwarder mapped
- [ ] **Belgique (BE)** — AliExpress list: unknown (provinces?) · postal code → exact · postal ranges name the province
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Luxembourg (LU)** — AliExpress list: unknown (cantons? maybe none) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Suisse (CH)** — AliExpress list: unknown (cantons?) · postal code → approx · PLZ cross cantons
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Allemagne (DE)** — AliExpress list: unknown (Länder?) · postal code → approx · PLZ regions cross Länder
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Espagne (ES)** — AliExpress list: unknown (provinces?) · postal code → exact · first two digits = province
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Italie (IT)** — AliExpress list: unknown (provinces?) · postal code → approx · CAP mostly names the province
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Pays-Bas (NL)** — AliExpress list: unknown (provinces?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Portugal (PT)** — AliExpress list: unknown (districts?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Autriche (AT)** — AliExpress list: unknown (Länder?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Irlande (IE)** — AliExpress list: unknown (counties?) · postal code → approx · Eircode routing key
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Royaume-Uni (GB)** — AliExpress list: counties + `Other` (forum reports) · postal code → none · `Other` may do
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **États-Unis (US)** — AliExpress list: states, required · postal code → exact · ZIP first three digits
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Canada (CA)** — AliExpress list: provinces, required · postal code → exact · first letter of the postal code
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Pologne (PL)** — AliExpress list: unknown (voivodeships?) · postal code → exact · first two digits
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Roumanie (RO)** — AliExpress list: unknown (counties?) · postal code → exact · first two digits
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Tchéquie (CZ)** — AliExpress list: unknown (regions?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Slovaquie (SK)** — AliExpress list: unknown (regions?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Hongrie (HU)** — AliExpress list: unknown (counties?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Slovénie (SI)** — AliExpress list: unknown · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Croatie (HR)** — AliExpress list: unknown (counties?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Bulgarie (BG)** — AliExpress list: unknown (oblasts?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Grèce (GR)** — AliExpress list: unknown (regions?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Chypre (CY)** — AliExpress list: unknown (districts?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Malte (MT)** — AliExpress list: unknown (maybe none) · postal code → none
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Monaco (MC)** — AliExpress list: unknown (maybe none) · postal code → none · one postal code
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Danemark (DK)** — AliExpress list: unknown (regions?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Suède (SE)** — AliExpress list: unknown (counties?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Norvège (NO)** — AliExpress list: unknown (counties?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Finlande (FI)** — AliExpress list: unknown (regions?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Estonie (EE)** — AliExpress list: unknown (counties?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Lettonie (LV)** — AliExpress list: unknown · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped
- [ ] **Lituanie (LT)** — AliExpress list: unknown (counties?) · postal code → approx
  - [ ] dropdown pasted
  - [ ] forwarder mapped

## French overseas departments

Not in the checkout's list, a shopper picks France. Saint-Barthélemy,
Saint-Martin, Saint-Pierre-et-Miquelon and the Pacific territories stay `FR` +
`Other`.

- [x] **Martinique (MQ)** — 972xx · country on AliExpress, State/Province free text and mandatory
- [ ] **Guadeloupe (GP)** — 971xx · mapped like Martinique, not confirmed on AliExpress
- [ ] **Guyane (GF)** — 973xx · mapped like Martinique, not confirmed on AliExpress
- [ ] **La Réunion (RE)** — 974xx · mapped like Martinique, not confirmed on AliExpress
- [ ] **Mayotte (YT)** — 976xx · mapped like Martinique, not confirmed on AliExpress

## When a country is mapped

- **exact:** a table like France's in `forwarding.rs`, a unit test in
  `crates/aliexpress/tests/forward.rs`, the spec's expectation if the e2e
  order ships there.
- **approx / none:** either `Other` when the list has it, or a State/Province
  field at checkout for that country, kept as a companion event on the order
  (the address type is frozen in `events.lock`).
