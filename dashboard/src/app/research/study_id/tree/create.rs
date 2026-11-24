use timada_research::{CreateIn, PlanCategory};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{Left, create_url, left_to_create, manageable_shop, reviewed};
use crate::app::research::study_id::require_owned;
use crate::app::research::{error_code, research};

#[derive(serde::Deserialize)]
pub struct CreateForm {
    /// The slug of a shop the owner may manage the catalog of.
    #[serde(default)]
    shop: String,
    /// The name of a shop to create from the study.
    #[serde(default)]
    name: String,
}

/// Asks for the study's tree to be created as draft categories: in a shop
/// its owner may manage the catalog of, or in a shop made from the study.
/// The request is recorded here and carried out off the page
/// (`crate::study_shop`): the shop first, then each category.
#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<CreateForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    let shop = form.shop.trim();
    let back = |error: Option<&str>, message: Option<&str>| {
        Ok(see_other(create_url(
            cx,
            &study.id,
            Some(shop).filter(|shop| !shop.is_empty()),
            error,
            message,
        )))
    };
    let review = reviewed(cx, &study).await?;
    if !review.is_ready() {
        return back(Some("tree_held_back"), None);
    }
    let (target, tenant) = if shop.is_empty() {
        match timada_tenancy::normalize_name(&form.name) {
            Ok(name) => (CreateIn::NewShop(name), None),
            Err(_) => return back(Some("invalid_name"), None),
        }
    } else {
        match manageable_shop(cx, &user_id, shop).await? {
            Some(tenant) => (CreateIn::Shop(tenant.id.clone()), Some(tenant)),
            None => return back(Some("shop_not_manageable"), None),
        }
    };
    // What a slug taken in the shop holds back is left out: asked for, it
    // would be refused.
    let plan: Vec<PlanCategory> = left_to_create(cx, &study, &review, tenant.as_ref())
        .await?
        .into_iter()
        .filter(|line| line.state == Left::Ready)
        .map(|line| line.plan)
        .collect();
    if plan.is_empty() {
        return back(Some("nothing_to_create"), None);
    }
    match research(cx)
        .request_creation(&user_id, &study.id, target, &plan)
        .await
    {
        Ok(_) => Ok(see_other(create_url(
            cx,
            &study.id,
            None,
            None,
            Some("The categories are being created as drafts."),
        ))),
        Err(err) if err.is_user_facing() => back(Some(error_code(&err)), None),
        Err(err) => Err(err.into()),
    }
}
