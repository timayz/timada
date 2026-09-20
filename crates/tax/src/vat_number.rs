//! VAT identification numbers of the European Union: reading one as a
//! business types it, and the port through which it is checked against the
//! EU's VIES registry. A sale to another member state is only exempt from
//! the seller's VAT when the buyer's number is *valid there*, so the answer
//! — and the consultation number VIES gives as proof — is worth keeping.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

/// A VAT number whose country prefix and shape are those of a member state:
/// `FR40303265045`. Whether it is *registered* is for a
/// [`VatNumberValidator`] to say.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VatNumber {
    prefix: String,
    number: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VatNumberError {
    #[error("a VAT number starts with its two-letter country prefix")]
    MissingPrefix,
    #[error("`{0}` is not the VAT prefix of a member state")]
    UnknownPrefix(String),
    #[error("`{0}` is not how a VAT number of that country is written")]
    Malformed(String),
}

/// `(VIES prefix, ISO country, shapes)`. In a shape `d` is a digit, `a` a
/// letter, `x` either, anything else itself.
const SHAPES: &[(&str, &str, &[&str])] = &[
    ("AT", "AT", &["Udddddddd"]),
    ("BE", "BE", &["dddddddddd"]),
    ("BG", "BG", &["ddddddddd", "dddddddddd"]),
    ("CY", "CY", &["dddddddda"]),
    ("CZ", "CZ", &["dddddddd", "ddddddddd", "dddddddddd"]),
    ("DE", "DE", &["ddddddddd"]),
    ("DK", "DK", &["dddddddd"]),
    ("EE", "EE", &["ddddddddd"]),
    // Greece goes by `EL` in VAT matters.
    ("EL", "GR", &["ddddddddd"]),
    ("ES", "ES", &["xdddddddx"]),
    ("FI", "FI", &["dddddddd"]),
    ("FR", "FR", &["xxddddddddd"]),
    ("HR", "HR", &["ddddddddddd"]),
    ("HU", "HU", &["dddddddd"]),
    ("IE", "IE", &["dxddddda", "ddddddda", "dddddddaa"]),
    ("IT", "IT", &["ddddddddddd"]),
    ("LT", "LT", &["ddddddddd", "dddddddddddd"]),
    ("LU", "LU", &["dddddddd"]),
    ("LV", "LV", &["ddddddddddd"]),
    ("MT", "MT", &["dddddddd"]),
    ("NL", "NL", &["dddddddddBdd"]),
    ("PL", "PL", &["dddddddddd"]),
    ("PT", "PT", &["ddddddddd"]),
    (
        "RO",
        "RO",
        &[
            "dd",
            "ddd",
            "dddd",
            "ddddd",
            "dddddd",
            "ddddddd",
            "dddddddd",
            "ddddddddd",
            "dddddddddd",
        ],
    ),
    ("SE", "SE", &["dddddddddddd"]),
    ("SI", "SI", &["dddddddd"]),
    ("SK", "SK", &["dddddddddd"]),
];

fn fits(shape: &str, number: &str) -> bool {
    shape.len() == number.len()
        && shape.chars().zip(number.chars()).all(|(s, c)| match s {
            'd' => c.is_ascii_digit(),
            'a' => c.is_ascii_uppercase(),
            'x' => c.is_ascii_digit() || c.is_ascii_uppercase(),
            literal => literal == c,
        })
}

/// A French number's two-character key, when it is numeric, derives from the
/// SIREN that follows it.
fn french_key_holds(number: &str) -> bool {
    let (key, siren) = number.split_at(2);
    match (key.parse::<u64>(), siren.parse::<u64>()) {
        (Ok(key), Ok(siren)) => (12 + 3 * (siren % 97)) % 97 == key,
        // Keys with letters follow another rule, left to VIES.
        _ => true,
    }
}

impl VatNumber {
    /// Reads a number as typed: spaces, dots and dashes are dropped, letters
    /// upper-cased. The country's shape is checked — and the key of a French
    /// number — not the registration.
    pub fn parse(input: &str) -> Result<Self, VatNumberError> {
        let compact: String = input
            .chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, '.' | '-' | '_'))
            .flat_map(char::to_uppercase)
            .collect();
        if compact.len() < 3 || !compact.is_char_boundary(2) {
            return Err(VatNumberError::MissingPrefix);
        }
        let (prefix, number) = compact.split_at(2);
        if !prefix.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(VatNumberError::MissingPrefix);
        }
        let Some((_, _, shapes)) = SHAPES.iter().find(|(known, ..)| *known == prefix) else {
            return Err(VatNumberError::UnknownPrefix(prefix.to_owned()));
        };
        let well_formed = shapes.iter().any(|shape| fits(shape, number))
            && (prefix != "FR" || french_key_holds(number));
        if !well_formed {
            return Err(VatNumberError::Malformed(compact));
        }
        Ok(Self {
            prefix: prefix.to_owned(),
            number: number.to_owned(),
        })
    }

    /// The prefix as VIES knows it: `EL` for Greece.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// What follows the prefix.
    pub fn number(&self) -> &str {
        &self.number
    }

    /// ISO 3166-1 alpha-2 of the member state that issued it: `GR` for `EL`.
    pub fn country_code(&self) -> &str {
        SHAPES
            .iter()
            .find(|(prefix, ..)| *prefix == self.prefix)
            .map_or(self.prefix.as_str(), |(_, country, _)| country)
    }
}

impl std::fmt::Display for VatNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.prefix, self.number)
    }
}

/// Whether `country_code` (ISO) is a member state of the EU's VAT area, as
/// far as VAT numbers go.
pub fn is_vat_member_state(country_code: &str) -> bool {
    SHAPES
        .iter()
        .any(|(_, country, _)| country.eq_ignore_ascii_case(country_code))
}

/// What the registry answered about a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VatCheck {
    pub valid: bool,
    /// The consultation number: the proof that the check was made, when the
    /// registry gives one (VIES does to a requester that names itself).
    pub consultation_ref: Option<String>,
    /// The name the number is registered under, when the member state
    /// discloses it.
    pub registered_name: Option<String>,
}

/// The registry could not answer — VIES, or the member state behind it, is
/// down: no verdict either way, to try again later.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("VAT registry unavailable: {0}")]
pub struct VatCheckUnavailable(pub String);

/// The result of a check, boxed so validators can be `dyn`.
pub type VatCheckFuture<'a> =
    Pin<Box<dyn Future<Output = Result<VatCheck, VatCheckUnavailable>> + Send + 'a>>;

pub trait VatNumberValidator: Send + Sync {
    fn check<'a>(&'a self, number: &'a VatNumber) -> VatCheckFuture<'a>;
}

/// No registry: a number that reads well is taken as valid, without proof.
/// For development, and for shops that verify by other means.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormatValidator;

impl VatNumberValidator for FormatValidator {
    fn check<'a>(&'a self, _number: &'a VatNumber) -> VatCheckFuture<'a> {
        Box::pin(async {
            Ok(VatCheck {
                valid: true,
                consultation_ref: None,
                registered_name: None,
            })
        })
    }
}

/// A registry for tests: numbers are valid unless said otherwise, and it can
/// be taken down.
#[derive(Debug, Clone, Default)]
pub struct FakeValidator {
    inner: Arc<Mutex<FakeRegistry>>,
}

#[derive(Debug, Default)]
struct FakeRegistry {
    unknown: Vec<String>,
    down: bool,
    checks: u32,
}

impl FakeValidator {
    fn registry(&self) -> std::sync::MutexGuard<'_, FakeRegistry> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The registry does not know that number.
    pub fn reject(&self, number: &VatNumber) {
        self.registry().unknown.push(number.to_string());
    }

    pub fn set_down(&self, down: bool) {
        self.registry().down = down;
    }

    /// How many checks were answered.
    pub fn checks(&self) -> u32 {
        self.registry().checks
    }
}

impl VatNumberValidator for FakeValidator {
    fn check<'a>(&'a self, number: &'a VatNumber) -> VatCheckFuture<'a> {
        Box::pin(async move {
            let mut registry = self.registry();
            if registry.down {
                return Err(VatCheckUnavailable("MS_UNAVAILABLE".to_owned()));
            }
            registry.checks += 1;
            let valid = !registry.unknown.contains(&number.to_string());
            Ok(VatCheck {
                valid,
                consultation_ref: valid.then(|| format!("FAKE-{}-{}", number, registry.checks)),
                registered_name: valid.then(|| "Registered Name GmbH".to_owned()),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_read_as_typed_and_checked_for_shape() {
        let number = VatNumber::parse(" fr 40 303 265 045 ").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(number.to_string(), "FR40303265045");
        assert_eq!((number.prefix(), number.number()), ("FR", "40303265045"));
        assert_eq!(number.country_code(), "FR");
        for good in [
            "DE123456789",
            "ATU12345678",
            "NL123456789B01",
            "IE1234567WA",
            "ESX1234567X",
            "RO12",
            "be0123.456.789",
        ] {
            assert!(VatNumber::parse(good).is_ok(), "{good}");
        }
        // Greece goes by EL; its ISO code is not a VAT prefix.
        let greek = VatNumber::parse("EL123456789").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(greek.country_code(), "GR");
        assert_eq!(
            VatNumber::parse("GR123456789"),
            Err(VatNumberError::UnknownPrefix("GR".into()))
        );

        assert_eq!(
            VatNumber::parse("12345"),
            Err(VatNumberError::MissingPrefix)
        );
        assert_eq!(VatNumber::parse("F"), Err(VatNumberError::MissingPrefix));
        assert_eq!(VatNumber::parse("é1"), Err(VatNumberError::MissingPrefix));
        assert_eq!(
            VatNumber::parse("GB123456789"),
            Err(VatNumberError::UnknownPrefix("GB".into()))
        );
        for bad in [
            "DE12345678",
            "DE1234567890",
            "ATX12345678",
            "NL123456789X01",
        ] {
            assert!(
                matches!(VatNumber::parse(bad), Err(VatNumberError::Malformed(_))),
                "{bad}"
            );
        }
        // A French key that does not derive from its SIREN.
        assert!(matches!(
            VatNumber::parse("FR41303265045"),
            Err(VatNumberError::Malformed(_))
        ));
        assert!(is_vat_member_state("gr") && is_vat_member_state("DE"));
        assert!(!is_vat_member_state("GB") && !is_vat_member_state("MQ"));
    }

    #[tokio::test]
    async fn the_fake_registry_answers_as_told() {
        let registry = FakeValidator::default();
        let known = VatNumber::parse("DE123456789").unwrap_or_else(|e| panic!("{e}"));
        let unknown = VatNumber::parse("DE999999999").unwrap_or_else(|e| panic!("{e}"));
        registry.reject(&unknown);

        let check = registry
            .check(&known)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(check.valid && check.consultation_ref.is_some());
        let check = registry
            .check(&unknown)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!check.valid && check.consultation_ref.is_none());
        registry.set_down(true);
        assert!(registry.check(&known).await.is_err());
        assert_eq!(registry.checks(), 2);
        assert!(FormatValidator.check(&unknown).await.is_ok_and(|c| c.valid));
    }
}
