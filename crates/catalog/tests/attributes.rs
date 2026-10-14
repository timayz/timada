mod common;

use common::*;
use timada_catalog::{CatalogError, MAX_ATTRIBUTE_VALUE_LEN, MAX_ATTRIBUTES};

#[tokio::test]
async fn a_fact_is_set_replaced_and_dropped() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cat Harness").await;
    cmd.set_attribute(ALICE, &product.id, " Material ", " Nylon ")
        .await
        .unwrap();
    cmd.set_attribute(ALICE, &product.id, "Weight", "120 g")
        .await
        .unwrap();
    let state = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(
        state
            .attributes
            .iter()
            .map(|a| (a.name.as_str(), a.value.as_str()))
            .collect::<Vec<_>>(),
        [("Material", "Nylon"), ("Weight", "120 g")],
        "in the order they were set"
    );

    cmd.set_attribute(ALICE, &product.id, "Material", "Cotton")
        .await
        .unwrap();
    let replaced = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(replaced.attributes.len(), 2, "the same name is one fact");
    assert_eq!(replaced.attributes[0].value, "Cotton");
    assert_eq!(
        replaced.attributes[0].name, "Material",
        "and keeps its place"
    );

    let version = replaced.aggregate_version;
    cmd.set_attribute(ALICE, &product.id, "Material", " Cotton ")
        .await
        .unwrap();
    assert_eq!(
        cmd.load_product(&product.id)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        version,
        "the same value writes nothing"
    );

    cmd.remove_attribute(ALICE, &product.id, "Material")
        .await
        .unwrap();
    cmd.remove_attribute(ALICE, &product.id, "Material")
        .await
        .expect("dropping twice is a no-op");
    assert_eq!(
        cmd.load_product(&product.id)
            .await
            .unwrap()
            .unwrap()
            .attributes
            .len(),
        1
    );
}

#[tokio::test]
async fn a_table_holds_thirty_facts_of_a_sensible_size() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cat Harness").await;
    for bad in [
        ("", "Nylon"),
        ("Material", ""),
        ("Material", &"v".repeat(MAX_ATTRIBUTE_VALUE_LEN + 1)),
    ] {
        let err = cmd.set_attribute(ALICE, &product.id, bad.0, bad.1).await;
        assert!(
            matches!(err, Err(CatalogError::InvalidAttribute)),
            "{bad:?}: {err:?}"
        );
    }

    for n in 0..MAX_ATTRIBUTES {
        cmd.set_attribute(ALICE, &product.id, &format!("Fact {n}"), "yes")
            .await
            .unwrap();
    }
    let full = cmd
        .set_attribute(ALICE, &product.id, "One more", "yes")
        .await;
    assert!(
        matches!(full, Err(CatalogError::TooManyAttributes)),
        "{full:?}"
    );
    cmd.set_attribute(ALICE, &product.id, "Fact 0", "no")
        .await
        .expect("but one already there still changes");
}
