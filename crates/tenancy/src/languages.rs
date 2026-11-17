//! The languages a shop may word its mails in: the list every context
//! shares ([`timada_core::languages`]).

pub use timada_core::languages::{LANGUAGES, is_language, language_name, language_rank};

/// The language a shop mails in until it picks its own, and the one the
/// built-in wording is written in.
pub const DEFAULT_LANGUAGE: &str = "en";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_wording_is_in_a_language_of_the_list() {
        assert_eq!(language_name(DEFAULT_LANGUAGE), Some("English"));
    }
}
