# Catalogue seeds

Snapshots a tenant can be filled with, through `timada-dashboard seed` (see
`docs/storefront.md`, "Seeding a catalogue"). A snapshot is the catalogue as the
catalog takes it: categories (a tree, each in or out of the menu), products with
their text, price or options and variants, facts, and the addresses of their
pictures. The pictures themselves are not here: the seed fetches them when it runs.

| File | Shop | Taken | Content |
|---|---|---|---|
| `chatounette.json.gz` | https://www.chatounette.com (WooCommerce, French cat shop) | see `fetched_at` inside | 60 categories, 1,330 products, ~5,350 variants |

The text and pictures belong to the shop they were taken from; the snapshot is here
to try the storefront examples against a real-sized catalogue, nothing more.

To take it again from the live shop (its public Store API), `make snapshot.chatounette`,
which runs the `woo-snapshot` binary of `crates/catalog-import` with the shop's menu
order and the categories its menu leaves out.
