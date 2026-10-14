//! `/t/{slug}/products/{product_id}/attributes`: the facts in a product's
//! table. The product page shows them; these handlers change them.

pub mod remove;
pub mod set;

#[derive(serde::Deserialize)]
pub struct AttributeForm {
    pub name: String,
    #[serde(default)]
    pub value: String,
}
