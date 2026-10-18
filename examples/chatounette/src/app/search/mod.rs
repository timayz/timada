//! The live search at `/search?q=`: not a page but the dropdown the masthead
//! shows as the shopper types, as chatounette.com does. The products that
//! match (as `/boutique` finds them), the categories whose name matches, the
//! link to all the results, then the shop's own pages under "Autres
//! résultats". The masthead's script fetches it and drops it under the box;
//! without the script the form still sends to `/boutique`.

use timada_storefront::{
    Category, ProductQuery, ProductSort, categories, product_image, products, search_categories,
    search_key,
};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use crate::app::{boutique, c, p, pages, price_line, support_client};

/// How many of each the dropdown shows.
const PRODUCTS: u32 = 4;
const CATEGORIES: usize = 3;
const PAGES_SHOWN: usize = 3;
/// Under this many characters the dropdown stays empty.
pub const MIN_CHARS: usize = 2;

#[topcoat::router::query_params(error = bad_request)]
struct SearchQuery {
    q: Option<String>,
}

#[derive(serde::Serialize)]
struct AllResults<'a> {
    q: &'a str,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<SearchQuery>(cx)?;
    let q = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| q.chars().count() >= MIN_CHARS)
        .unwrap_or_default()
        .to_owned();
    // Too short a query: an empty dropdown, which the script keeps hidden.
    let found = if q.is_empty() {
        None
    } else {
        Some(
            products(
                cx,
                &ProductQuery::default()
                    .search(&q)
                    .per_page(PRODUCTS)
                    .sort(ProductSort::Newest),
            )
            .await?,
        )
    };
    let tree = categories(cx).await?;
    // The view takes its data along, so the hits are copied out of the tree.
    let hits: Vec<Category> = search_categories(&tree, &q, CATEGORIES)
        .into_iter()
        .map(|node| node.category.clone())
        .collect();
    // A page matches on a word that starts with the query ("livrais" finds
    // "livraison"), not on any substring: "lit" would find every "politique".
    let key = search_key(&q);
    let starts_a_word = |text: &str| {
        !key.is_empty()
            && search_key(text).split(' ').any(|word| {
                word.trim_matches(|c: char| !c.is_alphanumeric())
                    .starts_with(&key)
            })
    };
    let own_pages: Vec<&pages::slug::StaticPage> = pages::slug::PAGES
        .iter()
        .chain(std::iter::once(&support_client::SEARCH_ENTRY))
        .filter(|page| {
            starts_a_word(page.title) || page.paragraphs.iter().any(|p| starts_a_word(p))
        })
        .take(PAGES_SHOWN)
        .collect();
    let nothing = found.as_ref().is_some_and(|found| found.items.is_empty())
        && hits.is_empty()
        && own_pages.is_empty();
    let all = href!(boutique::index)
        .query(AllResults { q: &q })
        .resolve(cx);
    // The support page is its own route; the others are static pages.
    let page_link = |slug: &str| {
        if slug == support_client::SEARCH_ENTRY.slug {
            href!(support_client::index).resolve(cx)
        } else {
            href!(pages::slug::index, pages::slug::Slug(slug.to_owned())).resolve(cx)
        }
    };

    Ok(view! {
        <div class="search-panel" data-search-results="" data-query=(&q)>
            if nothing {
                <p class="search-empty" data-search-empty="">"Aucun résultat pour « " (&q) " »"</p>
            } else {
                if let Some(found) = &found && !found.items.is_empty() {
                    <ul class="search-list search-products">
                        for product in &found.items {
                            <li>
                                <a class="search-row search-product" href=(href!(p::slug::index, p::slug::Slug(product.slug.clone()))) data-search-product=(&product.slug)>
                                    if let Some(image) = product.image() {
                                        product_image(image: image, attrs: attributes! { class="search-pic" })
                                    } else {
                                        <span class="search-pic search-pic-none" aria-hidden="true"></span>
                                    }
                                    <span class="search-text">
                                        <span class="search-name">(&product.name)</span>
                                        <span class="search-price" data-price="">(price_line(product))</span>
                                    </span>
                                </a>
                            </li>
                        }
                    </ul>
                }
                if !hits.is_empty() {
                    <ul class="search-list search-categories">
                        for category in &hits {
                            <li>
                                <a class="search-row" href=(href!(c::slug::index, c::slug::Slug(category.slug.clone()))) data-search-category=(&category.slug)>
                                    <span class="search-name">(&category.name)</span>
                                    <small class="search-kind">"Catégorie"</small>
                                </a>
                            </li>
                        }
                    </ul>
                }
                if let Some(found) = &found && !found.items.is_empty() {
                    <a class="search-all" href=(&all) data-search-all="">"Voir tous les résultats (" (found.total) ")"</a>
                }
                if !own_pages.is_empty() {
                    <p class="search-heading">"Autres résultats"</p>
                    <ul class="search-list search-pages">
                        for page in &own_pages {
                            <li>
                                <a class="search-row" href=(page_link(page.slug)) data-search-page=(page.slug)>
                                    <span class="search-name">(page.title)</span>
                                    <small class="search-kind">"Page"</small>
                                </a>
                            </li>
                        }
                    </ul>
                }
            }
        </div>
    })
}

/// The dropdown under the masthead's search box.
pub const SEARCH_CSS: &str = r#"
form.search { position: relative; }
.search-drop { position: absolute; left: 0; right: 0; top: 100%; z-index: 35; margin-top: .35rem; max-height: 70vh; overflow-y: auto; background: var(--paper); border: 1px solid var(--line); border-top: 2px solid var(--coral); border-radius: 0 0 var(--radius) var(--radius); box-shadow: var(--shadow); text-align: left; }
.search-list { list-style: none; margin: 0; padding: 0; }
.search-row { display: flex; align-items: center; gap: .75rem; padding: .6rem .85rem; border-bottom: 1px solid var(--line); color: var(--ink); text-decoration: none; font-size: .9rem; }
.search-row:hover, .search-row:focus-visible { background: var(--coral-wash); outline: none; }
.search-pic { flex: none; width: 3.5rem; height: 3.5rem; object-fit: cover; border-radius: .4rem; background: var(--soft); }
.search-text { display: flex; flex-direction: column; gap: .15rem; min-width: 0; }
.search-name { font-weight: 700; }
.search-price { color: var(--muted); }
.search-kind { margin-left: auto; flex: none; font-size: .7rem; letter-spacing: .04em; text-transform: uppercase; color: var(--muted); }
.search-all { display: block; padding: .7rem; background: var(--ink); color: #fff; text-align: center; text-decoration: none; text-transform: uppercase; font-size: .8rem; font-weight: 700; letter-spacing: .04em; }
.search-all:hover, .search-all:focus-visible { background: var(--coral-dark); outline: none; }
.search-heading { margin: 0; padding: .6rem .85rem .3rem; font-size: 1rem; font-weight: 700; color: var(--ink-soft); }
.search-empty { margin: 0; padding: .8rem .85rem; color: var(--muted); }
"#;

/// Fetches `/search?q=` as the shopper types and drops it under the box:
/// debounced, the previous request abandoned, the arrows walk the results,
/// Escape and a click elsewhere close it. Static and trusted.
pub const SEARCH_SCRIPT: &str = r##"
(() => {
const form = document.querySelector("form[data-search]");
if (!form) return;
const input = form.querySelector("input[name=q]");
const drop = form.querySelector("[data-search-drop]");
if (!input || !drop) return;
const minChars = Number(form.dataset.searchMin || 2);
let timer = null;
let controller = null;
let shown = "";

const hide = () => {
  drop.hidden = true;
  input.setAttribute("aria-expanded", "false");
};
const show = () => {
  if (!drop.firstElementChild) return;
  drop.hidden = false;
  input.setAttribute("aria-expanded", "true");
};
const links = () => [...drop.querySelectorAll("a")];

const load = async (q) => {
  controller?.abort();
  controller = new AbortController();
  try {
    const response = await fetch(`${form.dataset.search}?q=${encodeURIComponent(q)}`, {
      headers: { accept: "text/html" },
      signal: controller.signal,
    });
    if (!response.ok) throw new Error(response.statusText);
    const html = await response.text();
    if (input.value.trim() !== q) return;
    drop.innerHTML = html;
    shown = q;
    show();
  } catch (error) {
    if (error.name !== "AbortError") hide();
  }
};

input.addEventListener("input", () => {
  clearTimeout(timer);
  const q = input.value.trim();
  if (q.length < minChars) {
    controller?.abort();
    drop.innerHTML = "";
    shown = "";
    hide();
    return;
  }
  timer = setTimeout(() => load(q), 150);
});
input.addEventListener("focus", () => { if (shown && shown === input.value.trim()) show(); });
input.addEventListener("search", () => { if (!input.value) { drop.innerHTML = ""; shown = ""; hide(); } });

form.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    if (drop.hidden) return;
    event.preventDefault();
    // Focus first: focusing the box shows what it has for its value.
    input.focus();
    hide();
    return;
  }
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  if (drop.hidden) return;
  const items = links();
  if (!items.length) return;
  const at = items.indexOf(document.activeElement);
  event.preventDefault();
  if (event.key === "ArrowDown") {
    (at < 0 ? items[0] : items[at + 1] ?? input).focus();
  } else {
    (at < 0 ? items[items.length - 1] : items[at - 1] ?? input).focus();
  }
});
form.addEventListener("focusout", (event) => {
  if (event.relatedTarget && form.contains(event.relatedTarget)) return;
  hide();
});
document.addEventListener("click", (event) => {
  if (!form.contains(event.target)) hide();
});
})();
"##;
