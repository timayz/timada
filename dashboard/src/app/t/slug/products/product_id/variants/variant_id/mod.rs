//! One combination a product sells.

pub mod image;
pub mod price;
pub mod remove;
pub mod sku;

use topcoat::router::module_param;

module_param!(pub variant_id);

#[derive(serde::Deserialize)]
pub struct PriceForm {
    pub amount: String,
    pub currency: String,
}

#[derive(serde::Deserialize)]
pub struct SkuForm {
    #[serde(default)]
    pub sku: String,
}

#[derive(serde::Deserialize)]
pub struct ImageForm {
    #[serde(default)]
    pub image_id: String,
}
