//! `/t/{slug}/categories/{category_id}`: one category, read from its own
//! stream (so an edit shows at once), with the forms that change it.

pub mod ads;
pub mod archive;
pub mod describe;
pub mod menu;
pub mod move_to;
pub mod pin;
pub mod position;
pub mod publish;
pub mod questions;
pub mod rename;
pub mod seo;
pub mod translations;

use timada_catalog::{
    CategoryRow, CategoryState, MAX_CATEGORY_BODY_LEN, MAX_CATEGORY_DESCRIPTION_LEN,
    MAX_CATEGORY_NAME_LEN, MAX_META_DESCRIPTION_LEN, MAX_META_TITLE_LEN, MAX_PINNED_PRODUCTS,
    category_by_id, live_child_count,
};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, query_params,
    },
    view::{View, attributes, class, view},
};

use super::{Choice, error_code, rows_of};
use crate::app::auth::Flash;
use crate::app::t::slug::products::catalog;
use crate::app::t::slug::{TenantAccess, TenantNav, require_tenant, slug};
use crate::auth::user_labels;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::breadcrumb::{
    breadcrumb, breadcrumb_item, breadcrumb_link, breadcrumb_list, breadcrumb_page,
    breadcrumb_separator,
};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::CARD;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::label::label;
use crate::components::select::select;
use crate::components::textarea::textarea;
use crate::db::Db;
use crate::ui::translations::{parse_questions, questions_text};
use crate::ui::{
    LINK, Shell, document, fact, facts, format_time, notice, page_header, section_card,
};

module_param!(pub category_id);

#[topcoat::router::query_params(error = bad_request)]
struct CategoryQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn category_url(
    cx: &Cx,
    slug: &str,
    category_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        slug: slug.to_owned(),
        category_id: category_id.to_owned()
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The category named in the URL, or 404.
async fn require_category(cx: &Cx, access: &TenantAccess) -> Result<CategoryState> {
    let id = category_id(cx);
    catalog(access)?
        .load_category(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// What every action on a category shares: the member may manage products,
/// the category exists, and the outcome goes back to the category page.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_catalog::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), timada_catalog::CatalogError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let category_id = category_id(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = catalog(&access)?;
    match run(&gateway, &access.user_id, &category_id).await {
        Ok(()) => Ok(see_other(category_url(
            cx,
            &slug,
            &category_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(category_url(
            cx,
            &slug,
            &category_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let can_propose = access
        .can(cx, timada_tenancy::PERMISSION_PROPOSALS_PREPARE)
        .await?;
    let query = query_params::<CategoryQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let category = require_category(cx, &access).await?;
    let gateway = catalog(&access)?;
    let labels = user_labels(cx, std::slice::from_ref(&category.created_by)).await?;
    let created_by = labels
        .get(&category.created_by)
        .cloned()
        .unwrap_or_else(|| category.created_by.clone());
    // A shop read in one language has nothing to translate into.
    let translated = crate::app::t::slug::announcements::pages(&access)?
        .languages()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some_and(|languages| !languages.others.is_empty());
    let TenantAccess { tenant, .. } = access;
    let pool = context::<Db>(cx);
    // The whole tree: it carries the counts, and it says where this category
    // may move to (anything live that is not itself or one of its
    // descendants, with room for it below).
    let rows = rows_of(pool, &tenant.id).await?;
    // Filed directly under it, and what its page in the shop lists, as the
    // subscription stored them (the same numbers the tree shows).
    let (filed_here, on_its_page) = category_by_id(pool, &tenant.id, &category.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .map(|row| (row.product_count, row.subtree_count))
        .unwrap_or_default();
    // The breadcrumb, read off the tree already loaded. The parent comes
    // from the category itself, so a move shows at once.
    let ancestors: Vec<CategoryRow> = {
        let mut chain: Vec<CategoryRow> = Vec::new();
        let mut next = category.parent_id.as_deref();
        while let Some(parent_id) = next {
            if chain.len() > timada_catalog::MAX_CATEGORY_DEPTH
                || parent_id == category.id
                || chain.iter().any(|seen| seen.id == parent_id)
            {
                break;
            }
            let Some(parent) = rows.iter().find(|row| row.category.id == parent_id) else {
                break;
            };
            chain.push(parent.category.clone());
            next = parent.category.parent_id.as_deref();
        }
        chain.reverse();
        chain
    };
    let children = live_child_count(pool, &tenant.id, &category.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let descendants: Vec<&str> = {
        let mut ids = vec![category.id.as_str()];
        for row in &rows {
            if let Some(parent) = row.category.parent_id.as_deref()
                && ids.contains(&parent)
            {
                ids.push(row.category.id.as_str());
            }
        }
        ids
    };
    let parents: Vec<Choice> = rows
        .iter()
        .filter(|row| {
            !descendants.contains(&row.category.id.as_str())
                && row.depth + 2 <= timada_catalog::MAX_CATEGORY_DEPTH
        })
        .map(Choice::of)
        .collect();
    // The products put first, by their addresses: what the form shows and
    // takes. One deleted since is left out.
    let mut pinned = Vec::with_capacity(category.pinned.len());
    for id in &category.pinned {
        let product = gateway
            .load_product(id)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        pinned.extend(product.map(|product| product.slug));
    }
    let pinned_text = pinned.join("\n");
    let questions_text = questions_text(&category.questions);
    let slug = tenant.slug.clone();
    let editable = can_manage && !category.archived;
    let title = format!("{} · Categories · {}", category.name, tenant.name);
    let suggest =
        |kind: &str| crate::app::t::slug::proposals::new::new_url(cx, &slug, kind, &category.id);
    let suggest_url = suggest(timada_proposals::KIND_CATEGORY);
    let suggest_child_url = suggest(timada_proposals::KIND_NEW_CATEGORY);
    let can_draft =
        can_propose && !category.archived && context::<crate::writer::Writers>(cx).enabled();

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "categories"),
            breadcrumb(
                attrs: attributes! { class="mb-4" data-breadcrumb="" },
                breadcrumb_list(
                    breadcrumb_item(breadcrumb_link(attrs: attributes! { href=(href!(super::index, slug: slug.clone())) }, "All categories"))
                    for ancestor in &ancestors {
                        breadcrumb_separator()
                        breadcrumb_item(breadcrumb_link(attrs: attributes! { href=(href!(index, slug: slug.clone(), category_id: ancestor.id.clone())) }, (&ancestor.name)))
                    }
                    breadcrumb_separator()
                    breadcrumb_item(breadcrumb_page((&category.name)))
                )
            )
            page_header(
                title: &category.name,
                h1_attrs: attributes! { data-category-id=(&category.id) },
                if category.archived {
                    badge(variant: BadgeVariant::Secondary, attrs: attributes! { data-category-archived="" }, "Archived")
                } else if category.draft {
                    badge(variant: BadgeVariant::Secondary, attrs: attributes! { data-category-draft="" }, "Draft")
                }
                if translated {
                    <a class=(LINK) href=(href!(translations::index, slug: slug.clone(), category_id: category.id.clone())) data-category-translations="">"Translations"</a>
                }
                if can_propose && !category.archived {
                    <a class=(LINK) href=(&suggest_url) data-suggest-change="">"Suggest a change"</a>
                    <a class=(LINK) href=(&suggest_child_url) data-suggest-category="">"Suggest a category under it"</a>
                }
                if can_draft {
                    crate::app::t::slug::writer::draft_form(slug: &slug, kind: timada_proposals::KIND_CATEGORY, subject_id: &category.id)
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if children > 0 {
                <p class="mb-4 text-sm text-muted-foreground">
                    "Its page in the shop also lists what is filed under its sub-categories, so taking a product out of this category does not hide it there while it stays in one of them."
                </p>
            }

            section_card(
                title: Some("Overview"),
                facts(
                    fact(term: "Slug", <code class="text-xs">(&category.slug)</code>)
                    fact(
                        term: "Description",
                        attrs: attributes! { data-category-description="" class="whitespace-pre-line" },
                        (if category.description.is_empty() { "—" } else { category.description.as_str() })
                    )
                    fact(
                        term: "Below the products",
                        attrs: attributes! { data-category-body=(category.body.chars().count()) },
                        (if category.body.is_empty() { "—".to_owned() } else { format!("{} characters", category.body.chars().count()) })
                    )
                    fact(
                        term: "Questions answered",
                        attrs: attributes! { data-category-questions=(category.questions.len()) },
                        (category.questions.len())
                    )
                    fact(
                        term: "Shown first",
                        attrs: attributes! { data-category-pinned=(pinned.len()) },
                        (if pinned.is_empty() { "—".to_owned() } else { pinned.join(", ") })
                    )
                    fact(
                        term: "On its page",
                        attrs: attributes! { data-category-products=(on_its_page) },
                        (on_its_page) " product" (if on_its_page == 1 { "" } else { "s" })
                        if on_its_page != filed_here {
                            <span class="text-muted-foreground">", counting its sub-categories"</span>
                        }
                    )
                    fact(
                        term: "Filed here",
                        <a class=(LINK) data-category-direct=(filed_here) href=(href!(crate::app::t::slug::products::index, slug: slug.clone()).query(ProductsOfCategory { category: &category.id }).resolve(cx))>
                            (filed_here) " product" (if filed_here == 1 { "" } else { "s" })
                        </a>
                    )
                    fact(term: "Sub-categories", attrs: attributes! { data-category-children=(children) }, (children))
                    fact(term: "Position", (category.position))
                    fact(
                        term: "Shoppers",
                        attrs: attributes! { data-category-published=(if category.draft { "no" } else { "yes" }) },
                        if category.draft {
                            "A draft: its page answers nowhere, and the menu, the sitemap and the feeds leave it out"
                        } else {
                            "Published"
                        }
                    )
                    fact(
                        term: "Ads",
                        attrs: attributes! { data-category-ads=(if category.out_of_ads { "out" } else { "in" }) },
                        if category.out_of_ads {
                            "Kept out: the feed marks the products it is the main category of"
                        } else {
                            "Its products may be advertised"
                        }
                    )
                    fact(
                        term: "Shop menu",
                        attrs: attributes! { data-category-menu=(if category.hidden_from_menu { "hidden" } else { "shown" }) },
                        if category.hidden_from_menu {
                            "Hidden, with its sub-categories; its page stays open"
                        } else {
                            "Shown"
                        }
                    )
                    fact(term: "Created", (&created_by) ", " <time datetime=(category.created_at)>(format_time(category.created_at))</time>)
                    fact(term: "Changed", <time datetime=(category.updated_at)>(format_time(category.updated_at))</time>)
                )
            )

            if category.archived {
                <p class="mb-6 text-sm text-muted-foreground">
                    "An archived category cannot change any more. The products that were in it keep the membership until they leave it."
                </p>
            }

            if editable {
                <section class="mb-6 grid gap-4 md:grid-cols-2">
                    <form method="post" action=(href!(rename::rename, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-name" }, "Name")
                            input(attrs: attributes! {
                                id="category-name" name="name" type="text"
                                maxlength=(MAX_CATEGORY_NAME_LEN) required="" value=(&category.name)
                            })
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Rename")</div>
                    </form>
                    <form method="post" action=(href!(move_to::move_to, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-parent" }, "Parent")
                            select(
                                attrs: attributes! { id="category-parent" name="parent_id" },
                                <option value="" selected=(category.parent_id.is_none())>"Top level"</option>
                                for parent in &parents {
                                    <option value=(&parent.id) selected=(category.parent_id.as_deref() == Some(parent.id.as_str()))>
                                        (&parent.label)
                                    </option>
                                }
                            )
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Move")</div>
                    </form>
                    <form method="post" action=(href!(position::position, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-position" }, "Position among its siblings")
                            input(attrs: attributes! {
                                id="category-position" name="position" type="number"
                                min="0" max="10000" required="" value=(category.position)
                            })
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Set position")</div>
                    </form>
                    <form method="post" action=(href!(describe::describe, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-description" }, "Description")
                            textarea(
                                attrs: attributes! { id="category-description" name="description" rows="4" maxlength=(MAX_CATEGORY_DESCRIPTION_LEN) },
                                (&category.description)
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="category-body" }, "Buying guide, below the products")
                            field_description(attrs: attributes! { id="category-body-hint" }, "A short guide to choosing among these products, in Markdown: ## Heading, **bold**, - item, [link](/c/other). HTML is shown as text.")
                            textarea(
                                attrs: attributes! { id="category-body" name="body" rows="10" maxlength=(MAX_CATEGORY_BODY_LEN) aria-describedby="category-body-hint" },
                                (&category.body)
                            )
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save description")</div>
                    </form>
                    <form method="post" action=(href!(seo::seo, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-meta-title" }, "Search engines' title")
                            field_description(attrs: attributes! { id="category-meta-title-hint" }, "Empty uses the category's name.")
                            input(attrs: attributes! {
                                id="category-meta-title" name="meta_title" type="text" maxlength=(MAX_META_TITLE_LEN)
                                aria-describedby="category-meta-title-hint" value=(&category.meta_title)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="category-meta-description" }, "Search engines' description")
                            field_description(attrs: attributes! { id="category-meta-description-hint" }, "Empty uses the category's description.")
                            textarea(
                                attrs: attributes! {
                                    id="category-meta-description" name="meta_description" rows="3" maxlength=(MAX_META_DESCRIPTION_LEN)
                                    aria-describedby="category-meta-description-hint"
                                },
                                (&category.meta_description)
                            )
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save for search engines")</div>
                    </form>
                    <form method="post" action=(href!(questions::questions, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-questions" }, "Questions and answers")
                            field_description(attrs: attributes! { id="category-questions-hint" }, "What shoppers ask before buying here, shown below the guide. A question on one line, its answer on the lines after it, and an empty line before the next question.")
                            textarea(
                                attrs: attributes! { id="category-questions" name="questions" rows="10" aria-describedby="category-questions-hint" },
                                (&questions_text)
                            )
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save questions")</div>
                    </form>
                    <form method="post" action=(href!(pin::pin, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-pinned" }, "Products shown first")
                            field_description(attrs: attributes! { id="category-pinned-hint" }, "Up to " (MAX_PINNED_PRODUCTS) " products the category's page opens on, ahead of the others: one product a line (the last part of its address), in the order to show them. Only those published and filed here or below are shown.")
                            textarea(
                                attrs: attributes! { id="category-pinned" name="products" rows="4" aria-describedby="category-pinned-hint" class="font-mono text-xs" },
                                (&pinned_text)
                            )
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save the first products")</div>
                    </form>
                    <form method="post" action=(href!(menu::menu, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        label(
                            checkbox(attrs: attributes! { name="in_menu" value="on" checked=(!category.hidden_from_menu) })
                            "Show in the shop menu"
                        )
                        <p class="text-sm text-muted-foreground">"A hidden category takes its sub-categories out of the menu too. Its page and products stay reachable."</p>
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save menu visibility")</div>
                    </form>
                    <form method="post" action=(href!(ads::ads, slug: slug.clone(), category_id: category.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                        label(
                            checkbox(attrs: attributes! { name="out_of_ads" value="on" checked=(category.out_of_ads) })
                            "Keep its products out of ads"
                        )
                        <p class="text-sm text-muted-foreground">"For a category whose clicks cost more than a sale can pay. The shopping feed marks the products it is the main category of, on the label chosen on the feed's page, so campaigns exclude them. Its page and its place in search results are untouched."</p>
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save ads")</div>
                    </form>
                </section>

                <section class="flex flex-wrap items-center gap-3">
                    <form method="post" action=(href!(publish::publish, slug: slug.clone(), category_id: category.id.clone()))>
                        if category.draft {
                            <input type="hidden" name="published" value="yes">
                            button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Publish")
                        } else {
                            <input type="hidden" name="published" value="no">
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Keep back as a draft")
                        }
                    </form>
                    if children > 0 {
                        <p class="text-sm text-muted-foreground">"Archive or move its sub-categories before archiving it."</p>
                    } else {
                        <form method="post" action=(href!(archive::archive, slug: slug.clone(), category_id: category.id.clone()))>
                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Archive")
                        </form>
                    }
                </section>
            }
        )
    })
}

#[derive(serde::Deserialize)]
pub struct QuestionsForm {
    #[serde(default)]
    pub questions: String,
}

#[derive(serde::Deserialize)]
pub struct PinForm {
    #[serde(default)]
    pub products: String,
}

/// The products link of a category page.
#[derive(serde::Serialize)]
struct ProductsOfCategory<'a> {
    category: &'a str,
}

#[derive(serde::Deserialize)]
pub struct RenameForm {
    pub name: String,
}

#[derive(serde::Deserialize)]
pub struct SeoForm {
    #[serde(default)]
    pub meta_title: String,
    #[serde(default)]
    pub meta_description: String,
}

#[derive(serde::Deserialize)]
pub struct DescribeForm {
    #[serde(default)]
    pub description: String,
    /// Left alone when the form has no such field.
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct MoveForm {
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct PositionForm {
    pub position: u32,
}

#[derive(serde::Deserialize)]
pub struct MenuForm {
    /// An unticked checkbox is not sent at all.
    #[serde(default)]
    pub in_menu: Option<String>,
}
