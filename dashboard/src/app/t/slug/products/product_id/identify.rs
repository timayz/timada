use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use timada_catalog::Identify;

use super::{IdentifyForm, Tab, act};

/// Says who makes the product and under which numbers it is known; an
/// empty field clears it.
#[route(POST)]
pub async fn identify(cx: &Cx, Form(form): Form<IdentifyForm>) -> Result<SeeOther> {
    let cmd = Identify {
        brand: Some(form.brand),
        gtin: Some(form.gtin),
        mpn: Some(form.mpn),
        no_identifier: form.no_identifier.is_some(),
    };
    act(
        cx,
        Tab::Shopping,
        "Identifiers saved.",
        async |catalog, actor, id| catalog.identify_product(actor, id, cmd).await,
    )
    .await
}
