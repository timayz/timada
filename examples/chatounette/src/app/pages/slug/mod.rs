//! One of the shop's own pages (legal notice, terms of sale, about us), as
//! the merchant wrote and published it in the back office. Its HTML comes
//! from the library, cleaned; a draft, or a slug no page has, is a 404.

use timada_storefront::{menu, require_page};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param},
    view::{Unescaped, View, view},
};

use crate::app::document;

module_param!(pub slug);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = require_page(cx, path_param::<Slug>(cx)).await?;
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: &page.meta_title,
            description: Some(page.meta_description.as_str()),
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a> " / " (&page.title)
                </nav>
                <article class="page" data-page=(&page.slug)>
                    <h1>(&page.title)</h1>
                    // Cleaned by the library: no script, no attribute it did
                    // not allow, no picture from elsewhere.
                    (Unescaped::new_unchecked(page.html.clone()))
                </article>
            </div>
        )
    })
}
