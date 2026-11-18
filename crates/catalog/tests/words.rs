mod common;

use std::collections::HashMap;

use common::*;
use timada_catalog::{CatalogError, DefineVariant, MAX_WORD_LEN, OptionSelection};

fn terms(terms: &[&str]) -> Vec<String> {
    terms.iter().map(|term| (*term).to_owned()).collect()
}

#[tokio::test]
async fn a_word_is_worded_once_for_the_whole_shop() {
    let cmd = command().await;
    let asked = terms(&["Couleur", "Rose foncé", "Matière"]);
    assert!(cmd.load_words("en", &asked).await.unwrap().is_empty());

    cmd.word(ALICE, "en", " Couleur ", " Colour ")
        .await
        .unwrap();
    cmd.word(ALICE, "EN", "Rose foncé", "Dark pink")
        .await
        .unwrap();
    cmd.word(ALICE, "de", "Couleur", "Farbe").await.unwrap();
    // The entry of a language and a word, case and the space around aside.
    assert_eq!(
        cmd.load_words("en", &asked).await.unwrap(),
        HashMap::from([
            ("couleur".to_owned(), "Colour".to_owned()),
            ("rose foncé".to_owned(), "Dark pink".to_owned()),
        ])
    );
    assert_eq!(
        cmd.load_words("de", &terms(&["COULEUR"])).await.unwrap(),
        HashMap::from([("couleur".to_owned(), "Farbe".to_owned())])
    );

    // Worded again, and taken back: the main language's word stands.
    cmd.word(ALICE, "en", "couleur", "Color").await.unwrap();
    cmd.word(ALICE, "en", "Rose foncé", " ").await.unwrap();
    assert_eq!(
        cmd.load_words("en", &asked).await.unwrap(),
        HashMap::from([("couleur".to_owned(), "Color".to_owned())])
    );
    // Taking back what was never worded writes nothing.
    cmd.word(ALICE, "en", "Matière", "").await.unwrap();
}

#[tokio::test]
async fn a_word_needs_a_language_of_the_list_and_a_length() {
    let cmd = command().await;
    let refused = cmd.word(ALICE, "xx", "Couleur", "Colour").await;
    assert!(
        matches!(refused, Err(CatalogError::InvalidLanguage)),
        "{refused:?}"
    );
    let empty = cmd.word(ALICE, "en", " ", "Colour").await;
    assert!(matches!(empty, Err(CatalogError::InvalidWord)), "{empty:?}");
    let long = cmd
        .word(ALICE, "en", "Couleur", &"x".repeat(MAX_WORD_LEN + 1))
        .await;
    assert!(matches!(long, Err(CatalogError::InvalidWord)), "{long:?}");
}

#[tokio::test]
async fn a_product_lists_its_words_and_names_a_variant_in_them() {
    let cmd = command().await;
    let product = create_product(&cmd, "Panier").await;
    cmd.define_option(ALICE, &product.id, "Couleur", terms(&["Rose", "Bleu"]))
        .await
        .unwrap();
    cmd.define_option(ALICE, &product.id, "Taille", terms(&["Grand"]))
        .await
        .unwrap();
    cmd.set_attribute(ALICE, &product.id, "Matière", "Coton")
        .await
        .unwrap();
    cmd.set_attribute(ALICE, &product.id, "Lavable", "rose")
        .await
        .unwrap();
    let product = cmd.load_product(&product.id).await.unwrap().unwrap();
    // Each word once, case aside: "rose" is "Rose".
    assert_eq!(
        product.words(),
        terms(&[
            "Couleur", "Rose", "Bleu", "Taille", "Grand", "Matière", "Coton", "Lavable"
        ])
    );
    let colour = product.options[0].id.clone();
    let size = product.options[1].id.clone();
    let pick = |option_id: String, value: &str| OptionSelection {
        option_id,
        value: value.to_owned(),
    };
    let variant = cmd
        .define_variant(
            ALICE,
            &product.id,
            DefineVariant {
                selections: vec![pick(colour, "Rose"), pick(size, "Grand")],
                sku: None,
                amount_cents: 1200,
                currency: "EUR".into(),
                image_id: None,
            },
        )
        .await
        .unwrap();
    let product = cmd.load_product(&product.id).await.unwrap().unwrap();
    let variant = product.variant(&variant).unwrap();
    cmd.word(ALICE, "en", "Rose", "Pink").await.unwrap();
    let words = cmd.load_words("en", &product.words()).await.unwrap();
    // A value with no wording stays as the main language writes it.
    assert_eq!(
        product.variant_label_in(variant, &words).as_deref(),
        Some("Pink / Grand")
    );
}
