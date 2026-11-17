//! The filters of a listing, under "Afficher les filtres": a price band among seven fixed ones ("Filtrer par tarif")
//! and, for each option the merchant marked as a filter in the back office
//! ("Couleur", "Taille"; the colour alone until they choose) or fact of
//! the products' tables ("Matière"), a value among those the listed
//! products come in, each a link. A band keeps the
//! products a shopper could pay that much for (their range overlaps it).
//! All travel in the query string: `min_price` and `max_price` in whole
//! euros, an option under its folded name with a folded value
//! (`couleur=rose-fonce`). No JavaScript: the panel is a `<details>`, open
//! while a filter is on. The links are `nofollow`: a narrowed listing shows
//! the products of the plain one again, and search engines are kept out of
//! it.

use timada_storefront::{FacetValue, ProductQuery, facet_key, listing_filters};
use topcoat::{
    Result,
    context::Cx,
    router::request::original_uri,
    view::{View, view},
};

use super::money;

/// The bands, in cents, as the shop offers them.
pub const PRICE_BANDS: &[(i64, i64)] = &[
    (0, 5_000),
    (5_000, 7_000),
    (7_000, 10_000),
    (10_000, 15_000),
    (15_000, 25_000),
    (25_000, 50_000),
    (50_000, 200_000),
];

/// The options a listing is narrowed by until the merchant chooses theirs.
pub const DEFAULT_FILTERS: &[&str] = &["Couleur"];

/// What a listing's address already says with these names: an option
/// folding to one of them cannot be a filter.
const RESERVED: &[&str] = &["q", "page", "sort", "min_price", "max_price", "v"];

/// An option the shop's listings are narrowed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopFilter {
    /// As the merchant names the option: "Couleur".
    pub name: String,
    /// What carries it in an address: `couleur`.
    pub key: String,
}

/// The filters the names give, in order: one per key, none under a name an
/// address uses for something else.
fn filters_of<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<ShopFilter> {
    let mut filters: Vec<ShopFilter> = Vec::new();
    for name in names {
        let key = facet_key(name);
        if key.is_empty()
            || RESERVED.contains(&key.as_str())
            || filters.iter().any(|filter| filter.key == key)
        {
            continue;
        }
        filters.push(ShopFilter {
            name: name.to_owned(),
            key,
        });
    }
    filters
}

/// The options the shop's listings are narrowed by: the merchant's, or
/// [`DEFAULT_FILTERS`] while they never chose.
pub async fn shop_filters(cx: &Cx) -> Result<Vec<ShopFilter>> {
    Ok(match listing_filters(cx).await? {
        Some(names) => filters_of(names.iter().map(String::as_str)),
        None => filters_of(DEFAULT_FILTERS.iter().copied()),
    })
}

/// What a listing is narrowed by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    /// Whole euros.
    pub min_price: Option<u32>,
    pub max_price: Option<u32>,
    /// `(option key, value key)`, as [`facet_key`] writes them, one value
    /// an option, in the order the shop offers its filters.
    pub options: Vec<(String, String)>,
}

impl Filters {
    /// From the page's query string: the band its own parameters give, and
    /// a value for each of the `offered` options the address names; a blank
    /// one is none.
    pub fn read(
        cx: &Cx,
        min_price: Option<u32>,
        max_price: Option<u32>,
        offered: &[ShopFilter],
    ) -> Self {
        let pairs: Vec<(String, String)> = original_uri(cx)
            .query()
            .and_then(|query| serde_urlencoded::from_str(query).ok())
            .unwrap_or_default();
        Self::of(min_price, max_price, offered, &pairs)
    }

    fn of(
        min_price: Option<u32>,
        max_price: Option<u32>,
        offered: &[ShopFilter],
        pairs: &[(String, String)],
    ) -> Self {
        let options = offered
            .iter()
            .filter_map(|filter| {
                let (_, value) = pairs.iter().find(|(key, _)| *key == filter.key)?;
                let value = facet_key(value);
                (!value.is_empty()).then(|| (filter.key.clone(), value))
            })
            .collect();
        Self {
            min_price,
            max_price,
            options,
        }
    }

    /// `query` narrowed by these.
    pub fn apply(&self, query: ProductQuery) -> ProductQuery {
        let cents = |euros: Option<u32>| euros.map(|e| i64::from(e) * 100);
        let mut query = query.priced_between(cents(self.min_price), cents(self.max_price));
        for (option, value) in &self.options {
            query = query.with_option(option, value);
        }
        query
    }

    pub fn is_active(&self) -> bool {
        self.min_price.is_some() || self.max_price.is_some() || !self.options.is_empty()
    }

    /// The value the listing is narrowed to on the option `key`, if any.
    pub fn option(&self, key: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(option, _)| option == key)
            .map(|(_, value)| value.as_str())
    }

    /// The band of [`PRICE_BANDS`] these bounds name, if they name one.
    pub fn band(&self) -> Option<usize> {
        let (min, max) = (self.min_price?, self.max_price?);
        PRICE_BANDS
            .iter()
            .position(|(lo, hi)| i64::from(min) * 100 == *lo && i64::from(max) * 100 == *hi)
    }

    pub fn with_band(&self, index: usize) -> Self {
        let (lo, hi) = PRICE_BANDS[index];
        Self {
            min_price: Some((lo / 100) as u32),
            max_price: Some((hi / 100) as u32),
            options: self.options.clone(),
        }
    }

    pub fn without_band(&self) -> Self {
        Self {
            min_price: None,
            max_price: None,
            options: self.options.clone(),
        }
    }

    /// These, narrowed to `value` on the option `key` instead of whatever
    /// it was narrowed to there.
    pub fn with_option(&self, key: &str, value: &str) -> Self {
        let mut filters = self.clone();
        match filters.options.iter_mut().find(|(option, _)| option == key) {
            Some((_, was)) => *was = value.to_owned(),
            None => filters.options.push((key.to_owned(), value.to_owned())),
        }
        filters
    }

    pub fn without_option(&self, key: &str) -> Self {
        let mut filters = self.clone();
        filters.options.retain(|(option, _)| option != key);
        filters
    }

    /// `href`, a listing's address with its own parameters, with the
    /// options added to its query string.
    pub fn onto(&self, href: String) -> String {
        if self.options.is_empty() {
            return href;
        }
        let options = serde_urlencoded::to_string(&self.options).unwrap_or_default();
        let joint = if href.contains('?') { '&' } else { '?' };
        format!("{href}{joint}{options}")
    }
}

/// One link of the panel: where it leads, and whether it is the filter on.
/// An active link leads to the listing without it.
#[derive(Debug, Clone)]
pub struct FilterLink {
    /// What the link's `data-*` hook carries: `0-50` for a band, the value's
    /// key otherwise.
    pub key: String,
    pub label: String,
    pub href: String,
    pub active: bool,
}

/// The links of one option: its name, and the values the listing offers.
#[derive(Debug, Clone)]
pub struct FilterGroup {
    pub filter: ShopFilter,
    pub links: Vec<FilterLink>,
}

/// The panel, worked out by the page: `link` is the page's own address for
/// a filter state, at its first page.
#[derive(Debug, Clone)]
pub struct Panel {
    pub bands: Vec<FilterLink>,
    /// One group an option the listing has values of.
    pub groups: Vec<FilterGroup>,
    pub active: bool,
    /// The listing with no filter at all.
    pub clear: String,
}

impl Panel {
    pub fn build(
        filters: &Filters,
        offered: &[(ShopFilter, Vec<FacetValue>)],
        link: impl Fn(&Filters) -> String,
    ) -> Self {
        let band = filters.band();
        let bands = PRICE_BANDS
            .iter()
            .enumerate()
            .map(|(index, (lo, hi))| {
                let active = band == Some(index);
                FilterLink {
                    key: format!("{}-{}", lo / 100, hi / 100),
                    label: format!("De {} à {}", money(*lo, "EUR"), money(*hi, "EUR")),
                    href: link(&if active {
                        filters.without_band()
                    } else {
                        filters.with_band(index)
                    }),
                    active,
                }
            })
            .collect();
        let groups = offered
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(filter, values)| FilterGroup {
                filter: filter.clone(),
                links: values
                    .iter()
                    .map(|value| {
                        let active = filters.option(&filter.key) == Some(value.key.as_str());
                        FilterLink {
                            key: value.key.clone(),
                            label: format!("{} ({})", value.label, value.count),
                            href: link(&if active {
                                filters.without_option(&filter.key)
                            } else {
                                filters.with_option(&filter.key, &value.key)
                            }),
                            active,
                        }
                    })
                    .collect(),
            })
            .collect();
        Self {
            bands,
            groups,
            active: filters.is_active(),
            clear: link(&Filters::default()),
        }
    }
}

/// The panel under a listing's heading.
#[topcoat::view::component]
pub async fn filter_panel(cx: &Cx, panel: &Panel) -> Result<impl View> {
    Ok(view! {
        <details class="filters" data-filters="" open=(panel.active)>
            <summary>"Afficher les filtres"</summary>
            <div class="filters-body">
                <section>
                    <h3>"Filtrer par tarif"</h3>
                    <ul class="chips">
                        for band in &panel.bands {
                            <li>
                                <a data-price-band=(&band.key) href=(&band.href) rel="nofollow"
                                    aria-current=(band.active.then_some("true"))>(&band.label)</a>
                            </li>
                        }
                    </ul>
                </section>
                for group in &panel.groups {
                    <section data-filter-group=(&group.filter.key)>
                        <h3>"Filtrer par " (group.filter.name.to_lowercase())</h3>
                        <ul class="chips">
                            for value in &group.links {
                                <li>
                                    <a data-filter=(&group.filter.key) data-filter-value=(&value.key)
                                        href=(&value.href) rel="nofollow"
                                        aria-current=(value.active.then_some("true"))>(&value.label)</a>
                                </li>
                            }
                        </ul>
                    </section>
                }
                if panel.active {
                    <p class="filters-clear">
                        <a data-filters-clear="" href=(&panel.clear)>"Effacer les filtres"</a>
                    </p>
                }
            </div>
        </details>
    })
}

/// The filters a `GET` form must carry on, so sorting keeps them.
#[topcoat::view::component]
pub async fn filter_fields(cx: &Cx, filters: &Filters) -> Result<impl View> {
    Ok(view! {
        if let Some(min) = filters.min_price {
            <input type="hidden" name="min_price" value=(min)>
        }
        if let Some(max) = filters.max_price {
            <input type="hidden" name="max_price" value=(max)>
        }
        for (option, value) in &filters.options {
            <input type="hidden" name=(option) value=(value)>
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn the_filters_are_the_options_an_address_can_carry() {
        let filters = filters_of(["Taille", "Couleur", "taille", "Page", " ", "Matière & co"]);
        let keys: Vec<&str> = filters.iter().map(|filter| filter.key.as_str()).collect();
        // Once each, and never under a name the address uses itself.
        assert_eq!(keys, ["taille", "couleur", "matiere-&-co"]);
    }

    #[test]
    fn an_address_narrows_by_the_options_offered_and_no_other() {
        let offered = filters_of(["Couleur", "Taille"]);
        let read = Filters::of(
            Some(0),
            Some(50),
            &offered,
            &pairs(&[
                ("taille", "M"),
                ("matiere", "coton"),
                ("couleur", "Rose foncé"),
                ("page", "2"),
            ]),
        );
        assert_eq!(
            read.options,
            pairs(&[("couleur", "rose-fonce"), ("taille", "m")])
        );
        assert_eq!(read.option("taille"), Some("m"));
        assert!(read.is_active());
        // A blank value is none.
        let blank = Filters::of(None, None, &offered, &pairs(&[("couleur", " ")]));
        assert!(!blank.is_active());

        // One value an option: picking another replaces it.
        let other = read.with_option("couleur", "bleu");
        assert_eq!(other.option("couleur"), Some("bleu"));
        assert_eq!(other.options.len(), 2);
        assert_eq!(read.without_option("taille").options.len(), 1);
    }

    #[test]
    fn the_options_join_an_address_written_as_a_query() {
        let filters = Filters {
            options: pairs(&[("couleur", "rose-fonce"), ("matiere-&-co", "lin")]),
            ..Filters::default()
        };
        assert_eq!(
            filters.clone().onto("/c/tapis".to_owned()),
            "/c/tapis?couleur=rose-fonce&matiere-%26-co=lin"
        );
        assert_eq!(
            filters.onto("/c/tapis?sort=price_asc".to_owned()),
            "/c/tapis?sort=price_asc&couleur=rose-fonce&matiere-%26-co=lin"
        );
        assert_eq!(Filters::default().onto("/boutique".to_owned()), "/boutique");
    }
}
