//! Asks for the texts of a category's products to be drafted, one draft
//! per product, [`BATCH_MOST`] at a time.

use serde::Deserialize;
use timada_catalog::{ProductFilter, products_of_tenant};
use timada_core::page::PageFrom;
use timada_proposals::{KIND_CATEGORY, KIND_PRODUCT};
use timada_writer::latest_draft_of;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{Asked, ask, require_drafter, writer_url};
use crate::auth::now;
use crate::db::Db;
use crate::writer::Writers;

/// The most drafts one click asks for.
pub const BATCH_MOST: usize = 50;

/// The products read at a time, and the most pages walked to find
/// [`BATCH_MOST`] to draft.
const PAGE: i64 = 100;
const MOST_PAGES: usize = 10;

/// A product drafted since this long is passed over: asking again for a
/// category goes on where the last click stopped.
const RECENT_SECS: u64 = 24 * 60 * 60;

#[derive(Deserialize)]
pub struct BatchForm {
    category: String,
    #[serde(default)]
    with_category: Option<String>,
}

#[route(POST)]
pub async fn batch(cx: &Cx, Form(form): Form<BatchForm>) -> Result<SeeOther> {
    let access = require_drafter(cx).await?;
    let slug = access.tenant.slug.clone();
    if !context::<Writers>(cx).enabled() {
        return Ok(see_other(writer_url(cx, &slug, Some("writer_off"), None)));
    }
    let db = context::<Db>(cx);
    let tenant_id = access.tenant_id().to_owned();
    let since = now().saturating_sub(RECENT_SECS);
    // Whether a draft was asked for `id`: one drafted lately, or on its
    // way, is passed over.
    let request = async |kind: &str, id: &str| -> Result<bool> {
        let recent = latest_draft_of(db, &tenant_id, id, since)
            .await
            .map_err(topcoat::Error::from_anyhow)?
            .is_some_and(|row| row.is_open(now()) || !row.proposal_id.is_empty());
        if recent {
            return Ok(false);
        }
        match ask(cx, &access, kind, id, "").await {
            Ok(Asked::Requested) => Ok(true),
            Ok(_) => Ok(false),
            Err(err) if err.is_user_facing() => Ok(false),
            Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
        }
    };
    let (mut asked, mut passed) = (0, 0);
    let mut count = |was_asked: bool| {
        if was_asked {
            asked += 1;
        } else {
            passed += 1;
        }
    };
    let mut drafted = 0;
    if form.with_category.is_some() {
        count(request(KIND_CATEGORY, &form.category).await?);
    }
    let filter = ProductFilter::default().category(Some(&form.category));
    let mut from = None;
    'pages: for _ in 0..MOST_PAGES {
        let page = products_of_tenant(
            db,
            &tenant_id,
            &filter,
            from.as_ref().map_or(PageFrom::First, PageFrom::Older),
            PAGE,
        )
        .await
        .map_err(topcoat::Error::from_anyhow)?;
        for product in page.rows.iter().filter(|row| row.status != "archived") {
            if drafted >= BATCH_MOST {
                break 'pages;
            }
            let was_asked = request(KIND_PRODUCT, &product.id).await?;
            drafted += usize::from(was_asked);
            count(was_asked);
        }
        match page.older {
            Some(older) => from = Some(older),
            None => break,
        }
    }
    let message = match (asked, passed) {
        (0, 0) => "Nothing to draft: the category holds no product.".to_owned(),
        (0, _) => "Nothing to draft: every product was drafted in the last day or is on its way."
            .to_owned(),
        (_, 0) => format!("{asked} asked for: they will wait under Proposals as they are written."),
        _ => format!(
            "{asked} asked for: they will wait under Proposals as they are written. {passed} passed over, drafted in the last day or on their way."
        ),
    };
    Ok(see_other(writer_url(cx, &slug, None, Some(&message))))
}
