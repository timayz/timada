//! What a draft states that the shop's own data does not.
//!
//! A model asked to write from a product's facts may still state one it
//! was not given: a length, a voltage, a delay, a material. Nothing here
//! judges whether such a fact is true; it finds the ones the data does not
//! hold, so the member who reads the draft knows where to look.
//!
//! Two things are looked for, both cheap and neither clever: every figure
//! of the draft (a draft that says `30 cm` when no `30` is in the data),
//! and the words of a short list of materials and claims
//! ([`CLAIM_WORDS`]) that a draft uses and the data does not. A fact
//! worded otherwise is not found: the list is a net, not a proof.

use std::collections::BTreeSet;

use crate::value_object::{MAX_TO_CHECK, MAX_TO_CHECK_LEN};

/// Materials, and claims a shop answers for, in French and in English,
/// lowercase and singular. A draft that uses one the data does not hold is
/// marked.
pub const CLAIM_WORDS: [&str; 64] = [
    "coton",
    "cotton",
    "cuir",
    "leather",
    "laine",
    "wool",
    "soie",
    "silk",
    "lin",
    "linen",
    "polyester",
    "nylon",
    "acier",
    "steel",
    "inox",
    "aluminium",
    "aluminum",
    "bois",
    "wood",
    "bambou",
    "bamboo",
    "verre",
    "glass",
    "plastique",
    "plastic",
    "silicone",
    "céramique",
    "ceramic",
    "caoutchouc",
    "rubber",
    "titane",
    "titanium",
    "cuivre",
    "copper",
    "laiton",
    "brass",
    "velours",
    "velvet",
    "daim",
    "suede",
    "cachemire",
    "cashmere",
    "mérinos",
    "merino",
    "kevlar",
    "carbone",
    "carbon",
    "étanche",
    "waterproof",
    "imperméable",
    "homologué",
    "certifié",
    "certified",
    "garantie",
    "garanti",
    "warranty",
    "bio",
    "organic",
    "hypoallergénique",
    "hypoallergenic",
    "ignifugé",
    "fireproof",
    "bluetooth",
    "usb",
];

/// A figure as it compares: `1,50` and `1.5` are the same, `007` is `7`.
fn figure(digits: &str) -> String {
    let digits = digits.replace(',', ".");
    let (whole, fraction) = digits.split_once('.').unwrap_or((&digits, ""));
    let whole = whole.trim_start_matches('0');
    let fraction = fraction.trim_end_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    if fraction.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{fraction}")
    }
}

/// The figures of `text`, each with where it ends: digits, with one `.` or
/// `,` between two of them.
fn figures(text: &str) -> Vec<(String, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if !chars[at].1.is_ascii_digit() {
            at += 1;
            continue;
        }
        let start = chars[at].0;
        let mut end = at;
        let mut separated = false;
        while end < chars.len() {
            let c = chars[end].1;
            let decimal = !separated
                && (c == '.' || c == ',')
                && chars
                    .get(end + 1)
                    .is_some_and(|next| next.1.is_ascii_digit());
            if c.is_ascii_digit() {
                end += 1;
            } else if decimal {
                separated = true;
                end += 1;
            } else {
                break;
            }
        }
        let stop = chars.get(end).map_or(text.len(), |next| next.0);
        found.push((figure(&text[start..stop]), stop));
        at = end;
    }
    found
}

/// What follows a figure and reads with it: its unit (`cm`, `%`, `V`), with
/// or without a space before it.
fn unit(text: &str, from: usize) -> &str {
    let rest = &text[from..];
    let rest = rest
        .strip_prefix([' ', '\u{a0}', '\u{202f}'])
        .unwrap_or(rest);
    let end = rest
        .char_indices()
        .find(|(_, c)| !(c.is_alphabetic() || *c == '%' || *c == '°'))
        .map_or(rest.len(), |(at, _)| at);
    &rest[..end]
}

/// The words of `text`, lowercase.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

/// A word without the mark of its plural.
fn singular(word: &str) -> &str {
    word.strip_suffix('s').unwrap_or(word)
}

/// What `draft` states that `source` (everything the shop holds of the
/// subject, as one text) does not: its figures, each with its unit, then
/// the materials and claims it names, as the draft writes them, each once.
pub fn unfounded(draft: &str, source: &str) -> Vec<String> {
    let known: BTreeSet<String> = figures(source)
        .into_iter()
        .map(|(figure, _)| figure)
        .collect();
    let held: BTreeSet<String> = words(source)
        .map(|word| singular(&word).to_owned())
        .collect();
    let mut found: Vec<String> = Vec::new();
    let mut note = |what: String| {
        let what: String = what.chars().take(MAX_TO_CHECK_LEN).collect();
        if found.len() < MAX_TO_CHECK && !found.contains(&what) {
            found.push(what);
        }
    };
    for (figure, end) in figures(draft) {
        if known.contains(&figure) {
            continue;
        }
        match unit(draft, end) {
            "" => note(figure),
            unit => note(format!("{figure} {unit}")),
        }
    }
    for word in words(draft) {
        let bare = singular(&word);
        if CLAIM_WORDS.contains(&bare) && !held.contains(bare) {
            note(word);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_figure_the_data_does_not_hold_is_marked_with_its_unit() {
        let source = "Gants moto. Longueur: 30cm. Poids 0,50 kg. Livraison 7-12 jours.";
        assert!(
            unfounded(
                "Des gants de 30 cm, 0.5 kg, livrés en 7 à 12 jours.",
                source
            )
            .is_empty()
        );
        assert_eq!(
            unfounded(
                "Des gants de 32 cm pour 220V, garantis 2 ans, 100 % cuir.",
                source
            ),
            ["32 cm", "220 V", "2 ans", "100 %", "garantis", "cuir"]
        );
        // The same figure twice is one thing to check.
        assert_eq!(unfounded("45 mm, puis 45 mm.", ""), ["45 mm"]);
        assert_eq!(unfounded("Modèle 2024.", ""), ["2024"]);
    }

    #[test]
    fn a_material_or_a_claim_the_data_does_not_name_is_marked() {
        let source = "Gants en cuir, doublure coton";
        assert!(unfounded("Des gants en cuir doublés de coton.", source).is_empty());
        assert!(unfounded("Cuirs et cotons choisis.", source).is_empty());
        assert_eq!(
            unfounded(
                "Gants en cuir, étanches et homologués, paume Kevlar.",
                source
            ),
            ["étanches", "homologués", "kevlar"]
        );
        // A word that only looks like one is left alone.
        assert!(unfounded("Un boisé discret, un linge doux.", "").is_empty());
    }

    #[test]
    fn what_is_marked_is_bounded() {
        let draft: String = (1..100).map(|n| format!("{n}00 cm ")).collect();
        assert_eq!(unfounded(&draft, "").len(), MAX_TO_CHECK);
    }
}
