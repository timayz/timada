//! The product listing every browsing page shares — the catalogue, a
//! category, a brand, a search: filters read from the query string (plain
//! GET, no script), product cards, facets with their counts, sorting, pages.
//!
//! `marque` may repeat, which is why the query string is read by hand; a
//! value that makes no sense is ignored rather than refused — these URLs get
//! edited, shared and crawled.

use timada_catalog::{ListingPage, ListingQuery, ListingRow, ListingSort, search_listing};
use timada_core::Money;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, request::uri},
    view::{View, component, view},
};

use super::{catalog, format::money};
use crate::Store;

pub const PER_PAGE: u32 = 24;

/// What a page lists before any filter: a branch of the tree, a brand.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub category_id: Option<String>,
    pub brand_slug: Option<String>,
}

/// The shopper's filters, as the query string spells them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub q: Option<String>,
    pub brands: Vec<String>,
    /// In whole currency units, as typed.
    pub price_min: Option<i64>,
    pub price_max: Option<i64>,
    pub in_stock: bool,
    pub min_rating: Option<u8>,
    pub sort: ListingSort,
    pub page: u32,
}

const SORTS: [(&str, ListingSort, &str); 5] = [
    ("pertinence", ListingSort::Relevance, "Pertinence"),
    ("prix-croissant", ListingSort::PriceAsc, "Prix croissant"),
    (
        "prix-decroissant",
        ListingSort::PriceDesc,
        "Prix décroissant",
    ),
    ("note", ListingSort::Rating, "Meilleures notes"),
    ("nouveautes", ListingSort::Newest, "Nouveautés"),
];

impl Filters {
    pub fn parse(query: Option<&str>) -> Self {
        let mut filters = Self {
            page: 1,
            ..Self::default()
        };
        for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            match key.as_ref() {
                "q" => filters.q = Some(value.chars().take(100).collect()),
                "marque" if timada_core::slug::is_slug(value) && filters.brands.len() < 20 => {
                    filters.brands.push(value.to_owned());
                }
                "prix_min" => filters.price_min = value.parse().ok().filter(|p| *p >= 0),
                "prix_max" => filters.price_max = value.parse().ok().filter(|p| *p >= 0),
                "stock" => filters.in_stock = value == "1",
                "note" => filters.min_rating = value.parse().ok().filter(|n| (1..=4).contains(n)),
                "tri" => {
                    filters.sort = SORTS
                        .iter()
                        .find(|(code, ..)| *code == value)
                        .map_or(ListingSort::Relevance, |(_, sort, _)| *sort);
                }
                "page" => filters.page = value.parse().unwrap_or(1).max(1),
                _ => {}
            }
        }
        filters.brands.sort();
        filters.brands.dedup();
        filters
    }

    /// Whether anything but the page narrows or reorders the listing: such
    /// pages are endless variations of the plain one, not for search engines.
    pub fn is_narrowed(&self) -> bool {
        Self {
            page: self.page,
            ..Self::default()
        } != *self
    }

    /// `base` with these filters, on `page`.
    pub fn link(&self, base: &str, page: u32) -> String {
        let mut query = form_urlencoded::Serializer::new(String::new());
        if let Some(q) = &self.q {
            query.append_pair("q", q);
        }
        for brand in &self.brands {
            query.append_pair("marque", brand);
        }
        if let Some(min) = self.price_min {
            query.append_pair("prix_min", &min.to_string());
        }
        if let Some(max) = self.price_max {
            query.append_pair("prix_max", &max.to_string());
        }
        if self.in_stock {
            query.append_pair("stock", "1");
        }
        if let Some(stars) = self.min_rating {
            query.append_pair("note", &stars.to_string());
        }
        if let Some((code, ..)) = SORTS
            .iter()
            .find(|(_, sort, _)| *sort == self.sort && self.sort != ListingSort::Relevance)
        {
            query.append_pair("tri", code);
        }
        if page > 1 {
            query.append_pair("page", &page.to_string());
        }
        match query.finish() {
            query if query.is_empty() => base.to_owned(),
            query => format!("{base}?{query}"),
        }
    }
}

/// A listing ready to render, and what its page's `<head>` says of it.
pub struct Listing {
    pub base: String,
    pub filters: Filters,
    pub found: ListingPage,
    pub pages: u32,
    /// A brand's own page has no brand filter.
    pub brand_scoped: bool,
}

impl Listing {
    pub fn previous(&self) -> Option<String> {
        (self.filters.page > 1).then(|| self.filters.link(&self.base, self.filters.page - 1))
    }

    pub fn next(&self) -> Option<String> {
        (self.filters.page < self.pages)
            .then(|| self.filters.link(&self.base, self.filters.page + 1))
    }

    /// The address search engines should keep: the plain listing, page by
    /// page. Narrowed listings have none — they are not indexed.
    pub fn canonical(&self) -> Option<String> {
        (!self.filters.is_narrowed()).then(|| self.filters.link(&self.base, self.filters.page))
    }

    pub fn robots(&self) -> Option<&'static str> {
        self.filters.is_narrowed().then_some("noindex,follow")
    }
}

/// Searches what `scope` spans with the filters of the request's query
/// string. A page past the end is the last one.
pub async fn load_listing(cx: &Cx, base: String, scope: Scope) -> Result<Listing> {
    let store = app_context::<Store>(cx);
    let mut filters = Filters::parse(uri(cx).query());
    let brand_scoped = scope.brand_slug.is_some();
    let query_for = |filters: &Filters| ListingQuery {
        q: filters.q.clone(),
        category_id: scope.category_id.clone(),
        brand_slugs: match &scope.brand_slug {
            Some(brand) => vec![brand.clone()],
            None => filters.brands.clone(),
        },
        price_min_minor: filters.price_min.map(|p| p.saturating_mul(100)),
        price_max_minor: filters.price_max.map(|p| p.saturating_mul(100)),
        in_stock: filters.in_stock,
        min_rating: filters.min_rating,
        sort: filters.sort,
        limit: PER_PAGE,
        offset: (filters.page - 1).saturating_mul(PER_PAGE),
    };
    let mut found = search_listing(&store.db, &query_for(&filters)).await?;
    let pages = (found.total.max(0) as u32).div_ceil(PER_PAGE).max(1);
    if filters.page > pages {
        filters.page = pages;
        found = search_listing(&store.db, &query_for(&filters)).await?;
    }
    Ok(Listing {
        base,
        filters,
        found,
        pages,
        brand_scoped,
    })
}

fn rating_label(row: &ListingRow) -> Option<String> {
    row.rating_avg.map(|average| {
        format!(
            "{} / 5 ({} avis)",
            format!("{average:.1}").replace('.', ","),
            row.review_count
        )
    })
}

#[component]
async fn product_card(cx: &Cx, row: &ListingRow) -> Result<impl View> {
    let link = href!(
        catalog::product_page,
        catalog::ProductId(row.product_id.clone())
    )
    .resolve(cx);
    let price = money(&Money::new(row.price_minor, &row.currency));
    let rating = rating_label(row);
    let alt = row.thumbnail_alt.clone().unwrap_or_default();
    Ok(view! {
        <li class="product">
            <a href=(link.clone()) class="thumb" tabindex="-1" aria-hidden="true">
                match &row.thumbnail_url {
                    Some(url) => { <img src=(url.clone()) alt=(alt) width="240" height="240" loading="lazy" decoding="async"> }
                    None => { <span class="no-image">"Pas d'image"</span> }
                }
            </a>
            <h2><a href=(link)>(row.name.clone())</a></h2>
            <p class="muted">(row.brand_name.clone())</p>
            if let Some(rating) = &rating { <p>(rating.clone())</p> }
            <p class="price">(price)</p>
            if row.available > 0 {
                <p class="in-stock">"En stock"</p>
            } else {
                <p class="muted">"Rupture"</p>
            }
        </li>
    })
}

/// The filters, the products and the pages of a [`Listing`].
#[component]
pub async fn listing_view(listing: &Listing) -> Result<impl View> {
    let Listing {
        base,
        filters,
        found,
        pages,
        brand_scoped,
    } = listing;
    let facets = &found.facets;
    let brands: Vec<(String, String, bool)> = facets
        .brands
        .iter()
        .map(|brand| {
            (
                brand.slug.clone(),
                format!("{} ({})", brand.name, brand.count),
                filters.brands.contains(&brand.slug),
            )
        })
        .collect();
    let show_brands = !brand_scoped && brands.len() > 1;
    // Prices are filtered in whole units: the cheapest rounded down, the
    // dearest up, as placeholders.
    let (cheapest, dearest) = facets
        .price_range
        .map(|(min, max)| ((min / 100).to_string(), ((max + 99) / 100).to_string()))
        .unwrap_or_default();
    let ratings: Vec<(String, String, bool)> = facets
        .rated_at_least
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(stars, count)| {
            (
                stars.to_string(),
                format!("{stars} étoiles et plus ({count})"),
                filters.min_rating == Some(*stars),
            )
        })
        .collect();
    let sorts: Vec<(&str, &str, bool)> = SORTS
        .iter()
        .map(|(code, sort, label)| (*code, *label, *sort == filters.sort))
        .collect();
    let stock_label = format!("En stock uniquement ({})", facets.in_stock);
    let summary = match found.total {
        0 => "Aucun produit ne correspond.".to_owned(),
        1 => "1 produit".to_owned(),
        n => format!("{n} produits"),
    };
    let previous = listing.previous();
    let next = listing.next();
    let narrowed = filters.is_narrowed();

    Ok(view! {
        <div class="listing">
            <form method="get" action=(base.clone()) class="filters" aria-label="Filtrer les produits">
                <label>"Rechercher"
                    <input type="search" name="q" value=(filters.q.clone().unwrap_or_default())>
                </label>
                if show_brands {
                    <fieldset>
                        <legend>"Marque"</legend>
                        for (slug, brand_label, checked) in &brands {
                            <label class="choice">
                                <input type="checkbox" name="marque" value=(slug.clone()) checked=(*checked)>
                                <span>(brand_label.clone())</span>
                            </label>
                        }
                    </fieldset>
                }
                <fieldset>
                    <legend>"Prix"</legend>
                    <label>"Minimum"
                        <input type="number" name="prix_min" min="0" inputmode="numeric" placeholder=(cheapest) value=(filters.price_min.map(|p| p.to_string()).unwrap_or_default())>
                    </label>
                    <label>"Maximum"
                        <input type="number" name="prix_max" min="0" inputmode="numeric" placeholder=(dearest) value=(filters.price_max.map(|p| p.to_string()).unwrap_or_default())>
                    </label>
                </fieldset>
                <label class="choice">
                    <input type="checkbox" name="stock" value="1" checked=(filters.in_stock)>
                    <span>(stock_label)</span>
                </label>
                if !ratings.is_empty() {
                    <fieldset>
                        <legend>"Note des clients"</legend>
                        <label class="choice">
                            <input type="radio" name="note" value="" checked=(filters.min_rating.is_none())>
                            <span>"Toutes"</span>
                        </label>
                        for (stars, rating_label, checked) in &ratings {
                            <label class="choice">
                                <input type="radio" name="note" value=(stars.clone()) checked=(*checked)>
                                <span>(rating_label.clone())</span>
                            </label>
                        }
                    </fieldset>
                }
                <label>"Trier par"
                    <select name="tri">
                        for (code, sort_label, selected) in &sorts {
                            <option value=(*code) selected=(*selected)>(*sort_label)</option>
                        }
                    </select>
                </label>
                <button type="submit">"Filtrer"</button>
                if narrowed { <a href=(base.clone())>"Tout afficher"</a> }
            </form>
            <section aria-label="Produits">
                <p role="status" class="muted">(summary)</p>
                if !found.rows.is_empty() {
                    <ul class="products">
                        for row in &found.rows { product_card(row: row) }
                    </ul>
                }
                if *pages > 1 {
                    <nav aria-label="Pages" class="pager">
                        if let Some(previous) = &previous { <a href=(previous.clone()) rel="prev">"← Précédents"</a> }
                        <span>"Page " (filters.page.to_string()) " / " (pages.to_string())</span>
                        if let Some(next) = &next { <a href=(next.clone()) rel="next">"Suivants →"</a> }
                    </nav>
                }
            </section>
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_are_read_leniently_and_written_back_the_same() {
        let filters = Filters::parse(Some(
            "q=%C3%A9cran+24&marque=lg&marque=aoc&marque=LG!&marque=lg&prix_min=50&prix_max=abc\
             &stock=1&note=9&tri=prix-croissant&page=3&utm_source=x",
        ));
        assert_eq!(filters.q.as_deref(), Some("écran 24"));
        assert_eq!(filters.brands, ["aoc", "lg"]);
        assert_eq!((filters.price_min, filters.price_max), (Some(50), None));
        assert!(filters.in_stock);
        assert_eq!(filters.min_rating, None);
        assert_eq!(filters.sort, ListingSort::PriceAsc);
        assert_eq!(filters.page, 3);
        assert!(filters.is_narrowed());

        let link = filters.link("/c/ecrans", 2);
        assert_eq!(
            link,
            "/c/ecrans?q=%C3%A9cran+24&marque=aoc&marque=lg&prix_min=50&stock=1&tri=prix-croissant&page=2"
        );
        let again = Filters::parse(link.split_once('?').map(|(_, query)| query));
        assert_eq!(Filters { page: 3, ..again }, filters);

        // Nothing but a page: the plain listing.
        let plain = Filters::parse(Some("page=2&tri=pertinence&note="));
        assert!(!plain.is_narrowed());
        assert_eq!(plain.link("/", 1), "/");
        assert_eq!(plain.link("/", 2), "/?page=2");
        assert_eq!(Filters::parse(None).page, 1);
        assert_eq!(Filters::parse(Some("page=0")).page, 1);
    }
}
