use std::collections::BTreeMap;

use timada_catalog::OptionLabels;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::OptionId;
use crate::app::t::slug::products::product_id::act;

/// The prefix of the fields labelling one value. The form cannot know the
/// values in advance, so it posts `value:<value>` for each and the handler
/// reads them back out.
pub const VALUE_FIELD: &str = "value:";

/// Everything the form posts: `label` for the option, plus one
/// `value:<value>` a value. `BTreeMap` because the set of fields is the
/// option's, not the form's.
type LabelsForm = BTreeMap<String, String>;

/// Sets what the shop shows for an option and each of its values; an empty
/// field goes back to the name or the value. The names and values
/// themselves do not move, so an import that gave them still finds them.
#[route(POST)]
pub async fn labels(cx: &Cx, Form(form): Form<LabelsForm>) -> Result<SeeOther> {
    let option_id = path_param::<OptionId>(cx).to_owned();
    let empty_is_none = |text: &str| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    };
    let labels = OptionLabels {
        label: form.get("label").and_then(|label| empty_is_none(label)),
        values: form
            .iter()
            .filter_map(|(key, label)| {
                let value = key.strip_prefix(VALUE_FIELD)?;
                Some((value.to_owned(), empty_is_none(label)))
            })
            .collect(),
    };
    act(cx, "Option labels saved.", async |catalog, actor, id| {
        catalog.label_option(actor, id, &option_id, labels).await
    })
    .await
}
