use std::collections::BTreeMap;

use timada_catalog::{DefineVariant, OptionSelection, parse_amount};
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::OPTION_FIELD;
use crate::app::t::slug::products::product_id::act;

/// Everything the form posts: the fixed fields, plus one `option:<id>` a
/// menu. `BTreeMap` because the set of menus is the product's, not the
/// form's.
type DefineForm = BTreeMap<String, String>;

/// Picks a value from the form, trimmed, empty meaning absent.
fn field<'a>(form: &'a DefineForm, name: &str) -> Option<&'a str> {
    form.get(name).map(|v| v.trim()).filter(|v| !v.is_empty())
}

/// The selections the menus posted.
fn selections(form: &DefineForm) -> Vec<OptionSelection> {
    form.iter()
        .filter_map(|(key, value)| {
            let option_id = key.strip_prefix(OPTION_FIELD)?;
            Some(OptionSelection {
                option_id: option_id.to_owned(),
                value: value.trim().to_owned(),
            })
        })
        .collect()
}

#[route(POST)]
pub async fn define(cx: &Cx, Form(form): Form<DefineForm>) -> Result<SeeOther> {
    act(cx, "Variant added.", async |catalog, actor, id| {
        let amount = parse_amount(field(&form, "amount").unwrap_or_default())?;
        catalog
            .define_variant(
                actor,
                id,
                DefineVariant {
                    selections: selections(&form),
                    sku: field(&form, "sku").map(str::to_owned),
                    amount_cents: amount,
                    currency: field(&form, "currency").unwrap_or("EUR").to_owned(),
                    image_id: field(&form, "image_id").map(str::to_owned),
                },
            )
            .await
            .map(|_| ())
    })
    .await
}
