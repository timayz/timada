//! One picture of a product: its text, where it sits, and removing it.

pub mod alt;
pub mod down;
pub mod remove;
pub mod up;

use topcoat::router::module_param;

module_param!(pub image_id);

#[derive(serde::Deserialize)]
pub struct AltForm {
    #[serde(default)]
    pub alt: String,
}

/// Moves the picture `offset` places along the gallery, clamped to its ends,
/// and returns the whole new order. `None` when it does not move.
pub fn moved(
    images: &[timada_catalog::ProductImage],
    image_id: &str,
    offset: isize,
) -> Option<Vec<String>> {
    let at = images.iter().position(|image| image.id == image_id)?;
    let to = at.checked_add_signed(offset)?;
    if to >= images.len() {
        return None;
    }
    let mut order: Vec<String> = images.iter().map(|image| image.id.clone()).collect();
    let image = order.remove(at);
    order.insert(to, image);
    Some(order)
}
