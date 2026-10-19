# Catalogue seeds

Snapshots a tenant can be filled with, through `timada-dashboard seed` (see
`docs/storefront.md`, "Seeding a catalogue"). A snapshot is a directory: the catalogue
as the catalog takes it in `catalog.json.gz` (categories, a tree, each in or out of the
menu; products with their text, price or options and variants, facts, and their
pictures' addresses) and, under `pictures/`, the pictures themselves, one file per
address, so a seed needs no network.

Snapshots are not in git (this directory is ignored but for this file): with the
pictures they weigh hundreds of megabytes, and the text and pictures belong to the
shop they were taken from. Take one locally:

| Target | Shop | Content |
|---|---|---|
| `make snapshot.chatounette` → `seeds/chatounette/` | https://www.chatounette.com (WooCommerce, French cat shop) | ~60 categories, ~1,300 products, ~5,000 variants, ~8,000 pictures (~650 MB) |

It runs the `woo-snapshot` binary of `crates/catalog-import` over the shop's public
Store API with the shop's menu order and the categories its menu leaves out. What the
API leaves out is read off the shop's pages, two at a time and waiting when the shop
asks: the text below each category's products, and each product's "Description" tab
up to the shop's own "À propos" block (`--tab-stop-at`). Then it fetches the
pictures. Run again into the same directory, it refreshes the catalogue
and fetches only the pictures not there yet. A picture the fetch could not get is
left to the seed, which fetches it from the shop (`--no-pictures` leaves them all).

The snapshot is here to try the storefront examples against a real-sized catalogue,
nothing more.
