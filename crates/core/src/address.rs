use bitcode::{Decode, Encode};

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum Civility {
    #[default]
    Mr,
    Mrs,
}

/// A postal address as captured at checkout or in the customer's address book.
///
/// `country_code` is ISO 3166-1 alpha-2; French overseas territories use their
/// own codes (`GP`, `MQ`, `RE`, ...), which is how DOM-TOM shipping is told apart.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct Address {
    pub civility: Civility,
    pub first_name: String,
    pub last_name: String,
    pub line1: String,
    pub line2: Option<String>,
    pub postal_code: String,
    pub city: String,
    pub country_code: String,
    pub phone: Option<String>,
    pub mobile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("address field `{0}` is required")]
    Missing(&'static str),
    #[error("country code must be two uppercase letters, got `{0}`")]
    InvalidCountryCode(String),
}

impl Address {
    pub fn validate(&self) -> Result<(), AddressError> {
        let required = [
            ("first_name", &self.first_name),
            ("last_name", &self.last_name),
            ("line1", &self.line1),
            ("postal_code", &self.postal_code),
            ("city", &self.city),
        ];
        for (name, value) in required {
            if value.trim().is_empty() {
                return Err(AddressError::Missing(name));
            }
        }
        let code = &self.country_code;
        if code.len() != 2 || !code.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(AddressError::InvalidCountryCode(code.clone()));
        }
        Ok(())
    }

    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Address {
        Address {
            first_name: "Jonathan".into(),
            last_name: "Lapiquonne".into(),
            line1: "121, Avenue Tolosane".into(),
            line2: Some("Apt A21".into()),
            postal_code: "31520".into(),
            city: "Ramonville-Saint-Agne".into(),
            country_code: "FR".into(),
            ..Address::default()
        }
    }

    #[test]
    fn accepts_complete_address() {
        assert_eq!(valid().validate(), Ok(()));
    }

    #[test]
    fn rejects_missing_city() {
        let address = Address {
            city: "  ".into(),
            ..valid()
        };
        assert_eq!(address.validate(), Err(AddressError::Missing("city")));
    }

    #[test]
    fn rejects_bad_country_code() {
        let address = Address {
            country_code: "fra".into(),
            ..valid()
        };
        assert_eq!(
            address.validate(),
            Err(AddressError::InvalidCountryCode("fra".into()))
        );
    }
}
