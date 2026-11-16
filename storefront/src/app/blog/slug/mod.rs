//! One article of the blog at `/blog/{slug}`: its text as the merchant
//! published it, a table of contents from its headings, and the products
//! and categories it shows in between, drawn as the shop's own cards.

use timada_storefront::{
    ArticleBlock, ArticleLd, Category, Product, ProductQuery, absolute_url, article_ld,
    breadcrumb_ld, category_by_slug, menu, product_by_slug, product_image, products,
    require_article,
};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page},
    view::{Unescaped, View, attributes, view},
};

use super::published_on;
use crate::app::{SHOP_NAME, c, catalog_grid, document};

module_param!(pub slug);

/// How many of a category's products an article shows under its name.
const CATEGORY_PRODUCTS: u32 = 4;

/// One stretch of the article as the page draws it: text, or products
/// (one the article names, or a category's first few under its name).
struct Shown {
    /// Cleaned by the library: no script, no attribute it did not allow,
    /// no picture from elsewhere.
    html: Option<String>,
    category: Option<Category>,
    products: Vec<Product>,
}

/// Loads what each block shows. A product no longer on sale, or a category
/// since archived, leaves nothing: the text around it reads on.
async fn shown(cx: &Cx, blocks: Vec<ArticleBlock>) -> Result<Vec<Shown>> {
    let mut all = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            ArticleBlock::Html(html) => all.push(Shown {
                html: Some(html),
                category: None,
                products: Vec::new(),
            }),
            ArticleBlock::Product(slug) => {
                if let Some(product) = product_by_slug(cx, &slug).await? {
                    all.push(Shown {
                        html: None,
                        category: None,
                        products: vec![product],
                    });
                }
            }
            ArticleBlock::Category(slug) => {
                let Some(category) = category_by_slug(cx, &slug).await? else {
                    continue;
                };
                let query = ProductQuery::default()
                    .in_category(category.id.clone())
                    .per_page(CATEGORY_PRODUCTS);
                let products = products(cx, &query).await?.items;
                if !products.is_empty() {
                    all.push(Shown {
                        html: None,
                        category: Some(category),
                        products,
                    });
                }
            }
        }
    }
    Ok(all)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let mut article = require_article(cx, slug(cx)).await?;
    let nav = menu(cx).await?;
    let blocks = shown(cx, std::mem::take(&mut article.blocks)).await?;
    let path = href!(index, slug: article.slug.clone()).resolve(cx);
    let url = absolute_url(cx, &path);
    let home = absolute_url(cx, &href!(crate::app::index).resolve(cx));
    let blog = absolute_url(cx, &href!(super::index).resolve(cx));
    let image = article.cover.as_ref().map(|cover| cover.url.clone());
    let structured = [
        article_ld(&ArticleLd {
            title: &article.title,
            description: &article.meta_description,
            url: &url,
            image: image.as_deref(),
            published_at: article.published_at,
            updated_at: article.updated_at,
            publisher: SHOP_NAME,
            publisher_url: &home,
        }),
        breadcrumb_ld([
            ("Accueil", home.as_str()),
            ("Blog", blog.as_str()),
            (article.title.as_str(), url.as_str()),
        ]),
    ];
    // A table of contents for an article long enough to have parts.
    let contents = article.headings.len() >= 2;
    Ok(view! {
        document(
            title: &article.meta_title,
            description: Some(article.meta_description.as_str()).filter(|text| !text.is_empty()),
            canonical: Some(path.as_str()),
            image: image.as_deref(),
            published: Some(article.published_at),
            structured: &structured,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a>
                    " / " <a href=(href!(super::index))>"Blog"</a>
                    " / " (&article.title)
                </nav>
                <article class="page post" data-article=(&article.slug)>
                    <h1>(&article.title)</h1>
                    <p class="post-date">"Publié le " published_on(at: article.published_at)</p>
                    if !article.summary.is_empty() {
                        <p class="post-lead">(&article.summary)</p>
                    }
                    if let Some(cover) = &article.cover {
                        product_image(image: cover, large: true, attrs: attributes! { class="post-cover" })
                    }
                    if contents {
                        <nav class="toc" aria-labelledby="toc-title" data-toc="">
                            <h2 id="toc-title">"Sommaire"</h2>
                            <ol>
                                for heading in &article.headings {
                                    <li class=((heading.level > 2).then_some("sub"))>
                                        <a href=(format!("#{}", heading.id))>(&heading.text)</a>
                                    </li>
                                }
                            </ol>
                        </nav>
                    }
                    for block in &blocks {
                        if let Some(html) = &block.html {
                            (Unescaped::new_unchecked(html.clone()))
                        } else {
                            <aside class="post-products" data-article-products="">
                                if let Some(category) = &block.category {
                                    <p class="post-category">
                                        <a href=(href!(c::slug::index, c::slug::slug: category.slug.clone()))
                                            data-article-category=(&category.slug)>
                                            "Voir " (&category.name) " →"
                                        </a>
                                    </p>
                                }
                                catalog_grid(products: &block.products)
                            </aside>
                        }
                    }
                </article>
            </div>
        )
    })
}
