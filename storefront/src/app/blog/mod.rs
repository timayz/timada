//! The shop's blog at `/blog`: its articles, the most recent first, as the
//! merchant published them in the back office; one of them at
//! `/blog/{slug}`.

pub mod slug;

use timada_storefront::{ArticleCard, articles, civil_date, format_date, menu, product_image};
use topcoat::{
    Result,
    context::Cx,
    router::{error::not_found, href, page, query_params},
    view::{View, attributes, component, view},
};

use crate::app::document;

/// The list shows this many articles at once.
const PER_PAGE: u32 = 12;

#[topcoat::router::query_params(error = bad_request)]
struct BlogQuery {
    page: Option<u32>,
}

/// A link into the list. The first page is left out, so it has one address.
#[derive(serde::Serialize)]
struct BlogLink {
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
}

const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// A day as the shop writes it: `16 novembre 2026`, `1er mars 2026`.
pub fn long_date(at: i64) -> String {
    let (year, month, day) = civil_date(u64::try_from(at).unwrap_or_default());
    let month = MONTHS[(month as usize).clamp(1, 12) - 1];
    if day == 1 {
        format!("1er {month} {year}")
    } else {
        format!("{day} {month} {year}")
    }
}

/// The day an article came out, as a `<time>`.
#[component]
pub async fn published_on(cx: &Cx, at: i64) -> Result<impl View> {
    let machine = format_date(u64::try_from(at).unwrap_or_default());
    Ok(view! {
        <time datetime=(machine)>(long_date(at))</time>
    })
}

/// An article in a list: its cover, its title, its day and its summary.
#[component]
pub async fn article_card(cx: &Cx, article: &ArticleCard) -> Result<impl View> {
    let link = href!(slug::index, slug::slug: article.slug.clone()).resolve(cx);
    Ok(view! {
        <li class="post-card" data-article=(&article.slug)>
            if let Some(cover) = &article.cover {
                // The title beside it names the link: the picture adds nothing to say.
                <a class="post-pic" href=(&link) tabindex="-1" aria-hidden="true">
                    product_image(image: cover, attrs: attributes! { class="thumb" })
                </a>
            }
            <div class="post-body">
                <h2 class="post-title"><a href=(&link)>(&article.title)</a></h2>
                <p class="post-date">published_on(at: article.published_at)</p>
                if !article.summary.is_empty() {
                    <p class="post-summary">(&article.summary)</p>
                }
            </div>
        </li>
    })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let number = query_params::<BlogQuery>(cx)?.page.unwrap_or(1).max(1);
    let listed = articles(cx, number, PER_PAGE).await?;
    // A page past the last is no page; the first always answers, empty or
    // not.
    if listed.articles.is_empty() && number > 1 {
        return Err(not_found().into());
    }
    let nav = menu(cx).await?;
    let link = |number: u32| {
        href!(index)
            .query(BlogLink {
                page: (number > 1).then_some(number),
            })
            .resolve(cx)
    };
    let canonical = link(number);
    let title = if number > 1 {
        format!("Blog, page {number}")
    } else {
        "Blog".to_owned()
    };
    Ok(view! {
        document(
            title: &title,
            description: Some("Conseils, guides et nouveautés de la boutique."),
            canonical: Some(canonical.as_str()),
            // Nothing to read yet: nothing for a search engine to keep.
            noindex: listed.articles.is_empty(),
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a> " / Blog"
                </nav>
                <h1>"Blog"</h1>
                if listed.articles.is_empty() {
                    <p data-blog-empty="">"Aucun article pour le moment."</p>
                }
                <ul class="posts">
                    for article in &listed.articles {
                        article_card(article: article)
                    }
                </ul>
                if listed.pages > 1 {
                    <nav class="pages" aria-label="Pages">
                        if number > 1 { <a href=(link(number - 1))>"← Précédent"</a> }
                        <span>"Page " (number) " sur " (listed.pages)</span>
                        if number < listed.pages { <a href=(link(number + 1))>"Suivant →"</a> }
                    </nav>
                }
            </div>
        )
    })
}

#[cfg(test)]
mod tests {
    use super::long_date;

    #[test]
    fn a_day_reads_as_the_shop_writes_it() {
        assert_eq!(long_date(1_709_164_800), "29 février 2024");
        assert_eq!(long_date(1_709_251_200), "1er mars 2024");
    }
}
