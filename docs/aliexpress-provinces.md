# AliExpress: the State/Province of a forwarded order

AliExpress takes no order without a `province`, and checks it against its own
list for the country. The shop's addresses carry no region (street, postal
code, city, country, phone), so the forwarder derives one:

- **France:** the postal code's department, named as AliExpress lists them
  (`Rhone`, `Paris`, `Cote-d'Or`, `Corse-du-Sud`…), `Other` for a postal code
  the list lacks (overseas). Done, `french_department` in
  `crates/aliexpress/src/forwarding.rs`.
- **Every other country:** the city, a stand-in. It passes only where
  AliExpress has no list; elsewhere the order is refused with
  `Please select a State/Province/County`.

## Collecting a country's list

In AliExpress, open the account's address book, add an address, pick the
country, open the "State/Province" dropdown and copy its markup (the `<ul
role="listbox">`), as was done for France. Paste it in the issue or PR that
maps the country.

## Countries the storefront ships to

One line per country of the checkout's list. "Postal code →" says how well a
postal code alone names the subdivision AliExpress is likely to list:
**exact** can be mapped server-side like France, **approx** needs care at the
borders, **none** needs the shopper to pick it at checkout. Tick "list" once
the dropdown is pasted, "mapped" once the forwarder sends it.

| | Country | AliExpress list | Postal code → | Notes |
|---|---|---|---|---|
| - [x] list · [x] mapped | France (FR) | 94 metropolitan departments, unaccented, + `Other` | exact | #359 |
| - [ ] list · [ ] mapped | Belgique (BE) | unknown (provinces?) | exact | postal ranges name the province |
| - [ ] list · [ ] mapped | Luxembourg (LU) | unknown (cantons? maybe none) | approx | |
| - [ ] list · [ ] mapped | Suisse (CH) | unknown (cantons?) | approx | PLZ cross cantons |
| - [ ] list · [ ] mapped | Allemagne (DE) | unknown (Länder?) | approx | PLZ regions cross Länder |
| - [ ] list · [ ] mapped | Espagne (ES) | unknown (provinces?) | exact | first two digits = province |
| - [ ] list · [ ] mapped | Italie (IT) | unknown (provinces?) | approx | CAP mostly names the province |
| - [ ] list · [ ] mapped | Pays-Bas (NL) | unknown (provinces?) | approx | |
| - [ ] list · [ ] mapped | Portugal (PT) | unknown (districts?) | approx | |
| - [ ] list · [ ] mapped | Autriche (AT) | unknown (Länder?) | approx | |
| - [ ] list · [ ] mapped | Irlande (IE) | unknown (counties?) | approx | Eircode routing key |
| - [ ] list · [ ] mapped | Royaume-Uni (GB) | counties + `Other` (forum reports) | none | `Other` may do |
| - [ ] list · [ ] mapped | États-Unis (US) | states, required | exact | ZIP first three digits |
| - [ ] list · [ ] mapped | Canada (CA) | provinces, required | exact | first letter of the postal code |
| - [ ] list · [ ] mapped | Pologne (PL) | unknown (voivodeships?) | exact | first two digits |
| - [ ] list · [ ] mapped | Roumanie (RO) | unknown (counties?) | exact | first two digits |
| - [ ] list · [ ] mapped | Tchéquie (CZ) | unknown (regions?) | approx | |
| - [ ] list · [ ] mapped | Slovaquie (SK) | unknown (regions?) | approx | |
| - [ ] list · [ ] mapped | Hongrie (HU) | unknown (counties?) | approx | |
| - [ ] list · [ ] mapped | Slovénie (SI) | unknown | approx | |
| - [ ] list · [ ] mapped | Croatie (HR) | unknown (counties?) | approx | |
| - [ ] list · [ ] mapped | Bulgarie (BG) | unknown (oblasts?) | approx | |
| - [ ] list · [ ] mapped | Grèce (GR) | unknown (regions?) | approx | |
| - [ ] list · [ ] mapped | Chypre (CY) | unknown (districts?) | approx | |
| - [ ] list · [ ] mapped | Malte (MT) | unknown (maybe none) | none | |
| - [ ] list · [ ] mapped | Monaco (MC) | unknown (maybe none) | none | one postal code |
| - [ ] list · [ ] mapped | Danemark (DK) | unknown (regions?) | approx | |
| - [ ] list · [ ] mapped | Suède (SE) | unknown (counties?) | approx | |
| - [ ] list · [ ] mapped | Norvège (NO) | unknown (counties?) | approx | |
| - [ ] list · [ ] mapped | Finlande (FI) | unknown (regions?) | approx | |
| - [ ] list · [ ] mapped | Estonie (EE) | unknown (counties?) | approx | |
| - [ ] list · [ ] mapped | Lettonie (LV) | unknown | approx | |
| - [ ] list · [ ] mapped | Lituanie (LT) | unknown (counties?) | approx | |

## When a country is mapped

- **exact:** a table like France's in `forwarding.rs`, a unit test in
  `crates/aliexpress/tests/forward.rs`, the spec's expectation if the e2e
  order ships there.
- **approx / none:** either `Other` when the list has it, or a State/Province
  field at checkout for that country, kept as a companion event on the order
  (the address type is frozen in `events.lock`).
