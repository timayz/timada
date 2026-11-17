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

use crate::app::{document, words::t};

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

/// A day as the shop writes it: `16 novembre 2026`, `1er mars 2026`,
/// `November 16, 2026`.
pub fn long_date(cx: &Cx, at: i64) -> String {
    let (year, month, day) = civil_date(u64::try_from(at).unwrap_or_default());
    let month = match month {
        ..=1 => t!(blog_month_1),
        2 => t!(blog_month_2),
        3 => t!(blog_month_3),
        4 => t!(blog_month_4),
        5 => t!(blog_month_5),
        6 => t!(blog_month_6),
        7 => t!(blog_month_7),
        8 => t!(blog_month_8),
        9 => t!(blog_month_9),
        10 => t!(blog_month_10),
        11 => t!(blog_month_11),
        _ => t!(blog_month_12),
    };
    t!(blog_date, day: day, month: month.resolve(cx), year: year.to_string())
        .resolve(cx)
        .into_owned()
}

/// The day an article came out, as a `<time>`.
#[component]
pub async fn published_on(cx: &Cx, at: i64) -> Result<impl View> {
    let machine = format_date(u64::try_from(at).unwrap_or_default());
    Ok(view! {
        <time datetime=(machine)>(long_date(cx, at))</time>
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
        t!(blog_title_page, page: number)
    } else {
        t!(footer_blog)
    }
    .resolve(cx);
    let description = t!(blog_description).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            description: Some(&description),
            canonical: Some(canonical.as_str()),
            // Nothing to read yet: nothing for a search engine to keep.
            noindex: listed.articles.is_empty(),
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label=(t!(shell_crumbs)) data-crumbs="">
                    <a href=(href!(crate::app::index))>(t!(shell_home))</a> " / " (t!(footer_blog))
                </nav>
                <h1>(t!(footer_blog))</h1>
                if listed.articles.is_empty() {
                    <p data-blog-empty="">(t!(blog_empty))</p>
                }
                <ul class="posts">
                    for article in &listed.articles {
                        article_card(article: article)
                    }
                </ul>
                if listed.pages > 1 {
                    <nav class="pages" aria-label=(t!(blog_pages_label))>
                        if number > 1 { <a href=(link(number - 1))>(t!(blog_previous))</a> }
                        <span>(t!(blog_page_of, page: number, pages: listed.pages))</span>
                        if number < listed.pages { <a href=(link(number + 1))>(t!(blog_next))</a> }
                    </nav>
                }
            </div>
        )
    })
}

#[cfg(test)]
mod tests {
    use topcoat::{context::Cx, l10n::locale};

    use super::long_date;

    #[test]
    fn a_day_reads_as_the_shop_writes_it() {
        let cx = Cx::new().with(locale!("fr"));
        assert_eq!(long_date(&cx, 1_709_164_800), "29 février 2024");
        assert_eq!(long_date(&cx, 1_709_251_200), "1er mars 2024");
        let cx = Cx::new().with(locale!("en"));
        assert_eq!(long_date(&cx, 1_709_164_800), "February 29, 2024");
        assert_eq!(long_date(&cx, 1_709_251_200), "March 1, 2024");
    }
}
