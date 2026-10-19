use std::sync::Arc;

use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{CombinationForm, LinkId, back_to};
use crate::aliexpress::{AddResult, Aliexpress};
use crate::app::t::slug::require_tenant;
use crate::media::Media;

/// Adds a combination a read found as a variant (blank: every one offered).
#[route(POST)]
pub async fn combinations(cx: &Cx, Form(form): Form<CombinationForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link_id = path_param::<LinkId>(cx).to_owned();
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    let media = app_context::<Arc<Media>>(cx).clone();
    let store = access.executor()?.clone();
    let sku_ids: Vec<String> = Some(form.sku_id.trim().to_owned())
        .filter(|sku_id| !sku_id.is_empty())
        .into_iter()
        .collect();
    let result = aliexpress
        .add_combinations(
            &store,
            &access.tenant,
            media,
            &access.user_id,
            &link_id,
            &sku_ids,
        )
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(match result {
        AddResult::Added { added: 0, warnings } => {
            let message = warnings
                .first()
                .cloned()
                .unwrap_or_else(|| "Nothing to add.".to_owned());
            back_to(cx, &access, &form.back, None, Some(&message))
        }
        AddResult::Added { added, warnings } => {
            let mut message = if added == 1 {
                "1 combination added as a variant.".to_owned()
            } else {
                format!("{added} combinations added as variants.")
            };
            if !warnings.is_empty() {
                message.push_str(&format!(" {} could not be.", warnings.len()));
            }
            back_to(cx, &access, &form.back, None, Some(&message))
        }
        AddResult::Skipped => back_to(
            cx,
            &access,
            &form.back,
            Some("aliexpress_combinations_raced"),
            None,
        ),
        AddResult::NotLinked => back_to(
            cx,
            &access,
            &form.back,
            Some("aliexpress_not_connected"),
            None,
        ),
    })
}
