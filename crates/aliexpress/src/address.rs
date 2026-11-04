//! AliExpress's own names for places, from `aliexpress.ds.address.get`: an
//! order is refused unless its province and city are on them, and a shop's
//! address has only the postal code and the town the shopper typed.

use serde::Deserialize;
use serde_json::Value;

use crate::client::{AeError, LogisticsAddress};
use crate::model::text;

/// The answer's code for a country whose address form has no list.
const NO_LIST: &str = "801";

/// One country's places: provinces, their cities, and sometimes the cities'
/// towns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AddressTree(Vec<Node>);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Node {
    name: String,
    #[serde(default)]
    children: Vec<Node>,
}

/// Where AliExpress files a town.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    province: String,
    city: String,
    /// What the shopper typed, when the city does not say it.
    town: Option<String>,
}

impl AddressTree {
    /// The tree in the answer to `aliexpress.ds.address.get`, `None` for a
    /// country without a list, or why there is neither.
    pub fn from_response(body: &Value) -> Result<Option<Self>, AeError> {
        let response = body
            .get("aliexpress_ds_address_get_response")
            .unwrap_or(body);
        let result = response.get("result").unwrap_or(response);
        let read = |name: &str| result.get(name).map(text).unwrap_or_default();
        if !read("ret").eq_ignore_ascii_case("true") {
            if read("code") == NO_LIST {
                return Ok(None);
            }
            return Err(AeError::new("address", read("msg")));
        }
        let children = result
            .get("data")
            .and_then(|data| data.get("children"))
            .map(text)
            .unwrap_or_default();
        serde_json::from_str(&children)
            .map(Some)
            .map_err(|err| AeError::new("address", err.to_string()))
    }

    /// Whether `province` is on the list as something other than "Other":
    /// a province a delivery can be asked about.
    pub fn names_province(&self, province: &str) -> bool {
        province != OTHER && self.0.iter().any(|node| node.name == province)
    }

    /// The provinces, as an address form lists them.
    pub fn provinces(&self) -> Vec<&str> {
        self.0.iter().map(|node| node.name.as_str()).collect()
    }

    /// What a shopper may call the place they live in `province`, to suggest:
    /// its cities and towns, sorted and without "Other".
    pub fn places_in(&self, province: &str) -> Vec<&str> {
        let mut places: Vec<&str> = self
            .0
            .iter()
            .filter(|node| node.name == province)
            .flat_map(|node| &node.children)
            .flat_map(|city| std::iter::once(city).chain(&city.children))
            .map(|place| place.name.as_str())
            .filter(|name| *name != OTHER)
            .collect();
        places.sort_unstable();
        places.dedup();
        places
    }

    /// Puts an address in AliExpress's words: its city, and its province
    /// when the postal code did not name one, with the shopper's town kept
    /// in `address2` for the courier when the city does not say it. An
    /// address with no place on the list stays as it is, for AliExpress to
    /// refuse with its reason.
    pub fn apply(&self, address: &mut LogisticsAddress) {
        let Some(place) = self.place_of(&address.country, &address.province, &address.city) else {
            return;
        };
        address.province = place.province;
        address.city = place.city;
        if let Some(town) = place.town {
            if address.address2.is_empty() {
                address.address2 = town;
            } else {
                address.address2 = format!("{}, {town}", address.address2);
            }
        }
    }

    fn place_of(&self, country: &str, province: &str, town: &str) -> Option<Place> {
        let key = normalize(town);
        if key.is_empty() {
            return None;
        }
        let named = self
            .0
            .iter()
            .find(|node| node.name == province && province != OTHER);
        if named.is_none() && postal_code_decides(country) {
            return None;
        }
        let scope: Vec<&Node> = match named {
            Some(node) => vec![node],
            None => self.0.iter().filter(|node| node.name != OTHER).collect(),
        };
        // A city by that name before a town by that name: `Krakow` is a city
        // of one province and a village of another.
        let found = find(&scope, |city| normalize(&city.name) == key)
            .map(|(province, city)| Place {
                province: province.name.clone(),
                city: city.name.clone(),
                town: None,
            })
            .or_else(|| {
                let in_city = |city: &Node| city.children.iter().any(|t| normalize(&t.name) == key);
                find(&scope, in_city).map(|(province, city)| Place {
                    province: province.name.clone(),
                    city: city.name.clone(),
                    town: Some(town.trim().to_owned()),
                })
            });
        found.or_else(|| {
            let province = named?;
            province
                .children
                .iter()
                .any(|city| city.name == OTHER)
                .then(|| Place {
                    province: province.name.clone(),
                    city: OTHER.to_owned(),
                    town: Some(town.trim().to_owned()),
                })
        })
    }
}

/// The list's entry for what fits no other.
const OTHER: &str = "Other";

/// The first city that fits, unless cities of several provinces do: then
/// nothing says which.
fn find<'a>(scope: &[&'a Node], fits: impl Fn(&Node) -> bool) -> Option<(&'a Node, &'a Node)> {
    let mut hits = scope.iter().filter_map(|province| {
        province
            .children
            .iter()
            .find(|city| fits(city))
            .map(|city| (*province, city))
    });
    let first = hits.next()?;
    hits.next().is_none().then_some(first)
}

/// Whether [`crate::province_of`] read the province off the postal code, so
/// its "Other" means the code names none and a town by the same name
/// elsewhere must not overrule it.
fn postal_code_decides(country: &str) -> bool {
    matches!(country, "FR" | "BE" | "ES" | "CA" | "IT")
}

/// A place's name for comparing: `Lörrach` and `Loerrach`, `Forlì` and `forli`
/// are the same. AliExpress spells German umlauts out.
fn normalize(name: &str) -> String {
    let spelled: String = name
        .chars()
        .map(|c| match c {
            'ä' | 'Ä' => "ae".to_owned(),
            'ö' | 'Ö' => "oe".to_owned(),
            'ü' | 'Ü' => "ue".to_owned(),
            'ß' => "ss".to_owned(),
            other => other.to_string(),
        })
        .collect();
    deunicode::deunicode(&spelled)
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
