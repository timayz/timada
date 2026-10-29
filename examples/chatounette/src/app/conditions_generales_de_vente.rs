//! `/conditions-generales-de-vente`: « Conditions générales de vente », its text as the merchant published it.

use timada_storefront::{menu, require_page};
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, view},
};

use crate::app::own_page::{self, shop_page};

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = require_page(cx, own_page::CONDITIONS_GENERALES_DE_VENTE).await?;
    let nav = menu(cx).await?;
    Ok(view! { shop_page(page: &page, nav: &nav) })
}
