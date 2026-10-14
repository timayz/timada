//! `/t/{slug}/products/{product_id}/variants`: the combinations a product
//! sells. The product page shows them; these handlers change them.

pub mod define;
pub mod variant_id;

/// The prefix of the fields naming a value for one option. The form cannot
/// know the options in advance, so it posts `option:<id>` for each menu and
/// the handler reads them back out.
pub const OPTION_FIELD: &str = "option:";
