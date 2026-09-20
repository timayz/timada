//! URL slugs — lowercase ASCII words joined by hyphens — and the key names are
//! sorted by, so that `Écran` sits with the E's rather than after the Z's.

/// Turns a label into a slug: accents are folded (`Écran PC 24"` →
/// `ecran-pc-24`), everything that is not a letter or a digit separates words.
/// A label without any letter or digit gives an empty slug.
pub fn slugify(label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    for c in label.chars().flat_map(char::to_lowercase) {
        match fold(c) {
            Some(folded) => slug.push_str(folded),
            None if c.is_ascii_alphanumeric() => slug.push(c),
            None => {
                if !slug.is_empty() && !slug.ends_with('-') {
                    slug.push('-');
                }
            }
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// What a name is sorted by: lower case, accents folded, the rest as is.
/// Binary order on this key is alphabetical order for people —
/// `ecouteurs`, `Écran`, `Enceinte` — which SQLite's own collations are not.
pub fn sort_key(name: &str) -> String {
    let mut key = String::with_capacity(name.len());
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        match fold(c) {
            Some(folded) => key.push_str(folded),
            None => key.push(c),
        }
    }
    key
}

/// Whether `slug` is what [`slugify`] produces: safe to put in a URL as is.
pub fn is_slug(slug: &str) -> bool {
    !slug.is_empty() && slugify(slug) == slug
}

/// The ASCII spelling of the Latin letters European shop labels use.
fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
        'ç' => "c",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'ñ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "o",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'ý' | 'ÿ' => "y",
        'æ' => "ae",
        'œ' => "oe",
        'ß' => "ss",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{is_slug, slugify, sort_key};

    #[test]
    fn labels_become_url_words() {
        assert_eq!(slugify("Écran PC"), "ecran-pc");
        assert_eq!(
            slugify("  Périphériques & accessoires  "),
            "peripheriques-accessoires"
        );
        assert_eq!(slugify("Moniteur 23.8\" — 180 Hz"), "moniteur-23-8-180-hz");
        assert_eq!(slugify("Œuvres / Cœur"), "oeuvres-coeur");
        assert_eq!(slugify("STRASSE Straße"), "strasse-strasse");
        assert_eq!(slugify("→ ✓"), "");
    }

    #[test]
    fn only_clean_slugs_pass() {
        assert!(is_slug("ecran-pc-24"));
        for bad in [
            "",
            "Ecran",
            "écran",
            "ecran--pc",
            "-ecran",
            "ecran-",
            "a/b",
            "a b",
        ] {
            assert!(!is_slug(bad), "{bad}");
        }
    }

    #[test]
    fn names_sort_the_way_people_read_them() {
        let mut names = vec![
            "Zoom",
            "Écran",
            "enceinte",
            "Casque",
            "écouteurs",
            "Œil",
            "Ordinateur",
        ];
        names.sort_by_key(|name| sort_key(name));
        assert_eq!(
            names,
            [
                "Casque",
                "écouteurs",
                "Écran",
                "enceinte",
                "Œil",
                "Ordinateur",
                "Zoom"
            ]
        );
        assert_eq!(sort_key("  LG 27\" UltraFine "), "lg 27\" ultrafine");
    }
}
