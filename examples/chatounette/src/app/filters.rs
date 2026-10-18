//! The filters of a listing, as chatounette.com offers them under "Afficher
//! les filtres": a price band among seven fixed ones ("Filtrer par tarif")
//! and a colour among those the listed products come in ("Filtrer par
//! couleur"), each a link. A band keeps the products a shopper could pay
//! that much for (their range overlaps it); the colour is the "Couleur"
//! option of the products. Both travel in the query string as the live site
//! writes them: `min_price` and `max_price` in whole euros, `couleur` as a
//! folded key (`rose-fonce`). No JavaScript: the panel is a `<details>`,
//! open while a filter is on.

use timada_storefront::{FacetValue, ProductQuery, facet_key};
use topcoat::{
    Result,
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

/// The option the colour filter reads, as the merchant names it.
pub const COLOUR: &str = "Couleur";

/// What a listing is narrowed by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    /// Whole euros.
    pub min_price: Option<u32>,
    pub max_price: Option<u32>,
    /// A colour key, as [`facet_key`] writes it.
    pub couleur: Option<String>,
}

impl Filters {
    /// From the page's query string; a blank colour is none.
    pub fn new(min_price: Option<u32>, max_price: Option<u32>, couleur: Option<String>) -> Self {
        let couleur = couleur
            .as_deref()
            .map(facet_key)
            .filter(|key| !key.is_empty());
        Self {
            min_price,
            max_price,
            couleur,
        }
    }

    /// `query` narrowed by these.
    pub fn apply(&self, query: ProductQuery) -> ProductQuery {
        let cents = |euros: Option<u32>| euros.map(|e| i64::from(e) * 100);
        let query = query.priced_between(cents(self.min_price), cents(self.max_price));
        match &self.couleur {
            Some(key) => query.with_option(COLOUR, key),
            None => query,
        }
    }

    pub fn is_active(&self) -> bool {
        self.min_price.is_some() || self.max_price.is_some() || self.couleur.is_some()
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
            couleur: self.couleur.clone(),
        }
    }

    pub fn without_band(&self) -> Self {
        Self {
            min_price: None,
            max_price: None,
            couleur: self.couleur.clone(),
        }
    }

    pub fn with_colour(&self, key: &str) -> Self {
        Self {
            couleur: Some(key.to_owned()),
            ..self.clone()
        }
    }

    pub fn without_colour(&self) -> Self {
        Self {
            couleur: None,
            ..self.clone()
        }
    }
}

/// One link of the panel: where it leads, and whether it is the filter on.
/// An active link leads to the listing without it.
#[derive(Debug, Clone)]
pub struct FilterLink {
    /// What the link's `data-*` hook carries: `0-50` for a band, the colour
    /// key otherwise.
    pub key: String,
    pub label: String,
    pub href: String,
    pub active: bool,
}

/// The panel, worked out by the page: `link` is the page's own address for
/// a filter state, at its first page.
#[derive(Debug, Clone)]
pub struct Panel {
    pub bands: Vec<FilterLink>,
    pub colours: Vec<FilterLink>,
    pub active: bool,
    /// The listing with no filter at all.
    pub clear: String,
}

impl Panel {
    pub fn build(
        filters: &Filters,
        colours: &[FacetValue],
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
        let colours = colours
            .iter()
            .map(|value| {
                let active = filters.couleur.as_deref() == Some(value.key.as_str());
                FilterLink {
                    key: value.key.clone(),
                    label: format!("{} ({})", value.label, value.count),
                    href: link(&if active {
                        filters.without_colour()
                    } else {
                        filters.with_colour(&value.key)
                    }),
                    active,
                }
            })
            .collect();
        Self {
            bands,
            colours,
            active: filters.is_active(),
            clear: link(&Filters::default()),
        }
    }
}

/// The panel under a listing's heading.
#[topcoat::view::component]
pub async fn filter_panel(panel: &Panel) -> Result<impl View> {
    Ok(view! {
        <details class="filters" data-filters="" open=(panel.active)>
            <summary>"Afficher les filtres"</summary>
            <div class="filters-body">
                <section>
                    <h3>"Filtrer par tarif"</h3>
                    <ul class="chips">
                        for band in &panel.bands {
                            <li>
                                <a data-price-band=(&band.key) href=(&band.href)
                                    aria-current=(band.active.then_some("true"))>(&band.label)</a>
                            </li>
                        }
                    </ul>
                </section>
                if !panel.colours.is_empty() {
                    <section>
                        <h3>"Filtrer par couleur"</h3>
                        <ul class="chips">
                            for colour in &panel.colours {
                                <li>
                                    <a data-colour=(&colour.key) href=(&colour.href)
                                        aria-current=(colour.active.then_some("true"))>(&colour.label)</a>
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
pub async fn filter_fields(filters: &Filters) -> Result<impl View> {
    Ok(view! {
        if let Some(min) = filters.min_price {
            <input type="hidden" name="min_price" value=(min)>
        }
        if let Some(max) = filters.max_price {
            <input type="hidden" name="max_price" value=(max)>
        }
        if let Some(colour) = &filters.couleur {
            <input type="hidden" name="couleur" value=(colour)>
        }
    })
}
