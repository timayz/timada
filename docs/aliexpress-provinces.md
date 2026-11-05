# AliExpress: the State/Province of a forwarded order

AliExpress takes no order without a `province`. Where it has a list for the
country, it checks the value against that list. The shop's addresses carry no
region (street, postal code, city, country, phone), so the forwarder derives
one in `province_of`, `crates/aliexpress/src/forwarding.rs`.

## Reading the lists

`aliexpress.ds.address.get` with `countryCode` answers the address form's tree,
signed with the app key and no buyer token. It is free of charge and read-only.
Its `children` is a JSON string, provinces first, then cities and sometimes
towns. A country without a list answers `ret: false`, code 801,
`Address Exception`. The lists below were read on 2026-11-04.

## The countries

**Mapped from the postal code** (`Other` when the postal code names none):

| Country | AliExpress lists |
|---|---|
| France (FR) | 96 departments + `Other`, unaccented |
| Belgique (BE) | `Region Bruxelles-Capitale`, `Region Flamande`, `Region Wallonne` + `Other` |
| Espagne (ES) | 52 provinces + `Other`, some in their own spelling (`Alacant`, `Castello`, `Guipuzcoa`, `Vizcaya`) |
| Canada (CA) | 13 provinces and territories + `Other`; `X…` sends `Other` |
| Italie (IT) | 20 regions, no `Other`; a postal code naming none sends the city |

**`Other`**, as a postal code does not reliably name the entry:

| Country | AliExpress lists |
|---|---|
| Allemagne (DE) | 16 Länder + `Other` |
| Autriche (AT) | 9 Länder + `Other` |
| Pays-Bas (NL) | 12 provinces + `Other` |
| Portugal (PT) | 20 districts and islands + `Other` |
| Royaume-Uni (GB) | 19 nations and territories + `Other` |
| Tchéquie (CZ) | 14 regions + `Other` |
| Lettonie (LV) | 46 municipalities + `Other` |

**The town decides.** A list without `Other`, and no postal code that names the
entry. These send the city unless the lookup below finds the town:

| Country | AliExpress lists |
|---|---|
| États-Unis (US) | 50 states + DC |
| Pologne (PL) | 16 voivodeships |
| Suisse (CH) | 26 cantons |
| Estonie (EE) | 15 counties |
| Lituanie (LT) | 10 counties |

**No list:** the city, as a free text. Bulgarie, Chypre, Croatie, Danemark,
Finlande, Grèce, Hongrie, Irlande, Luxembourg, Malte, Monaco, Norvège, Roumanie,
Slovaquie, Slovénie, Suède. The free text was seen on the site for Martinique
only, the others are read from the API's answer.

**French overseas departments:** the checkout files them under France,
AliExpress lists them as countries, so `delivery_country` turns the postal code
into one for the address and the delivery. None has a list, so each takes the
city. Martinique (MQ) was seen on the site; Guadeloupe (GP), Guyane (GF),
La Réunion (RE) and Mayotte (YT) answer as it does. Saint-Barthélemy,
Saint-Martin, Saint-Pierre-et-Miquelon and the Pacific territories stay `FR` +
`Other`.

## The city, and the town lookup

AliExpress checks `city` as well: a real Italian order with `Aci Castello` was
refused with `Please enter a City`, although that town is on the list, under
Sicilia → Catania, and so was a French one with `Alès`, listed as `Ales`. So the forwarder reads the country's tree when it places an
order, kept for a day, and `AddressTree::apply` puts the address in its words (if
the tree cannot be read, nothing is placed and the order page says to try again):

- The town is looked for in the province the postal code named, or in every
  province where the postal code names none.
- A city by that name is sent as it is. A town sends its parent as the city, and
  stays in `address2` for the courier, since the order has no field for a town.
- A town on the list in several provinces, with no province named, stays as it
  was: `Freiburg` is in two Länder.
- In a named province without the town, its `Other` city takes it, if it has one.
  Italy's provinces have none, so that address goes as typed.

Delivery is asked with the province too. The docs say it is for local-to-local
products: an item shipped from Germany answered
`DELIVERY_NOT_AVAILABLE_TO_YOUR_ADDRESS` to Germany without one. The lookup does
not know every name the address form lists (`Gard` answered
`INVALID_PROVINCE_NAME`), so it is asked again without a name it refuses.

## The checkout's province list

In a shop connected to AliExpress, the Chatounette checkout asks as AliExpress's own
form does when the country has a list: the province, its city, then that city's town
if it has any (`GET /store/places`, then `timada_storefront::places`). The postal code
picks the province until the shopper does, the pick is `ShippingAddress::region`, and
it wins over the postal code when the order is forwarded. The order's city is the
last place picked, in AliExpress's spelling, on its documents too. The city is typed
only for a province without cities, or under the list's `Other`, which the lookup
above then files. Where a province has no `Other` (the US, Poland, Switzerland,
Estonia, Lithuania, Italy), a town off the list cannot be picked. In AT, BE, CH, CZ,
EE and LT that list holds districts, not towns: the form calls it "Province / district"
and asks for the town with the street, as AliExpress's forms for Belgium and
Switzerland do, so such an order's city is the district. No list, no script
or no answer: the city is typed and the lookup does what it can. So does an express
payment, whose address comes from the wallet.

## Not proven by a real order

- That `Catania` passes where `Aci Castello` was refused.
- That the province cures the German item's refusal.
- That AliExpress takes `Other`, as a province outside France or as a city.
- That an address needs what the site's form asks: a street of at least 5
  characters and a separate house number. The order has `address` and `address2`
  only, and the shopper's whole line goes in `address`.
