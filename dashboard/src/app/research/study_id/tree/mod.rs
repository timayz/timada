//! `/research/{study_id}/tree`: the category tree a study's owner builds
//! by hand from its kept keywords. A keyword becomes a parent category, a
//! child of one, or is parked for later; each category shows the figures of
//! its keyword, the price its clicks are read against and whether campaigns
//! are to leave it out, and what stands between it and its creation in a
//! shop. The tree is read from the study's stream, the figures from the
//! shared read model.

pub mod create;
pub mod place;
pub mod price;
pub mod words;

use timada_catalog::MAX_CATEGORY_NAME_LEN;
use timada_core::page::{Cursor, PageFrom};
use timada_research::{
    Channel, KeywordFigures, PlanCategory, ReviewedCategory, StudyState, Thresholds, TreeProblem,
    TreeReview, USUAL_PARENTS, keyword_figures, keywords_of_outcome, outcome_counts, review_tree,
};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, TenantState, tenants_of_user};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, component, view},
};

use super::{require_owned, study_id, study_url};
use crate::app::tenants::tenancy;
use crate::cluster::LocalReads;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::pagination::{
    pagination, pagination_content, pagination_item, pagination_next, pagination_previous,
};
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::components::textarea::textarea;
use crate::db::Db;
use crate::research::format_ratio;
use crate::ui::{
    LINK, PageTab, Shell, Tone, document, empty_row, icons, notice, page_header, page_tabs,
    section_card, status_badge,
};

/// Where a row's form sends its keyword: among the parent categories.
pub(crate) const PARENT: &str = "parent";
/// Under the parent category whose keyword follows.
pub(crate) const UNDER: &str = "under:";
/// Among the keywords parked for later.
pub(crate) const PARK: &str = "park";
/// Out of the tree and of the parked ones.
pub(crate) const REMOVE: &str = "remove";

/// The page shows this many kept keywords at once.
const PAGE_SIZE: i64 = 50;

/// The lists the kept keywords are read by.
const LISTS: [(&str, &str); 2] = [("main", "Main"), ("long_tail", "Long tail")];

#[topcoat::router::query_params(error = bad_request)]
struct TreeQuery {
    /// Which of the kept keywords are listed ([`LISTS`]); the main ones
    /// without one.
    list: Option<String>,
    after: Option<String>,
    before: Option<String>,
    /// The slug of the shop the page shows what would be created in.
    shop: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct TreeLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    list: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shop: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

/// The key of the list a parameter names, when it names one.
fn list_of(list: Option<&str>) -> Option<&'static str> {
    LISTS
        .iter()
        .map(|(key, _)| *key)
        .find(|key| Some(*key) == list)
}

/// The tree's page of a study, on one of the lists of kept keywords.
pub(crate) fn tree_url(
    cx: &Cx,
    study_id: &str,
    list: Option<&str>,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, study_id: study_id.to_owned())
        .query(TreeLink {
            list: list_of(list).filter(|list| *list != LISTS[0].0),
            shop: None,
            after: None,
            before: None,
            error,
            message,
        })
        .resolve(cx)
}

/// The tree's page of a study at what it creates in a shop, with the shop
/// looked at when one is.
pub(crate) fn create_url(
    cx: &Cx,
    study_id: &str,
    shop: Option<&str>,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    let url = href!(index, study_id: study_id.to_owned())
        .query(TreeLink {
            list: None,
            shop,
            after: None,
            before: None,
            error,
            message,
        })
        .resolve(cx);
    format!("{url}#create")
}

/// The study's tree, checked against the figures the read model holds.
pub(crate) async fn reviewed(cx: &Cx, study: &StudyState) -> Result<TreeReview> {
    let in_tree: Vec<String> = study
        .tree
        .categories
        .iter()
        .map(|category| category.keyword.clone())
        .collect();
    let figures = keyword_figures(context::<Db>(cx), &study.id, &in_tree)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let seeds: Vec<String> = study.seeds.iter().map(|seed| seed.seed.clone()).collect();
    Ok(review_tree(
        &study.tree,
        &seeds,
        &study.inputs,
        &Thresholds::default(),
        &figures,
    ))
}

/// The shop that goes by `slug`, when `user_id` may manage its catalog and
/// it is not suspended.
pub(crate) async fn manageable_shop(
    cx: &Cx,
    user_id: &str,
    slug: &str,
) -> Result<Option<TenantState>> {
    Ok(tenancy(cx)
        .load_tenant_by_slug(slug)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|tenant| !tenant.is_suspended() && tenant.can(user_id, PERMISSION_PRODUCTS_MANAGE)))
}

/// Whether a category left to create can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Left {
    Ready,
    /// A category or a product of the shop goes by its slug.
    Taken,
    /// The category above it cannot be created.
    UnderTaken,
}

/// One category of the tree that the shop does not have yet.
pub(crate) struct LeftLine {
    pub plan: PlanCategory,
    pub state: Left,
}

/// The categories of the tree the study has not created, each as it would
/// be created, parents before their children. With the shop they are for,
/// one whose slug is taken there is said so, and its children with it:
/// shown before anything is created, not after.
pub(crate) async fn left_to_create(
    cx: &Cx,
    study: &StudyState,
    review: &TreeReview,
    tenant: Option<&TenantState>,
) -> Result<Vec<LeftLine>> {
    let catalog = tenant.map(|tenant| {
        timada_catalog::Command::new(context::<LocalReads>(cx).0.clone(), &tenant.id)
    });
    let mut lines: Vec<LeftLine> = Vec::new();
    let mut held: Vec<&str> = Vec::new();
    for category in &review.categories {
        if study.is_created(&category.keyword) {
            continue;
        }
        let under_held = category
            .parent
            .as_deref()
            .is_some_and(|parent| held.contains(&parent));
        let taken = match &catalog {
            Some(catalog) if !under_held => catalog
                .category_slug_taken(&category.slug)
                .await
                .map_err(topcoat::Error::from_anyhow)?,
            _ => false,
        };
        let state = if under_held {
            Left::UnderTaken
        } else if taken {
            Left::Taken
        } else {
            Left::Ready
        };
        if state != Left::Ready {
            held.push(&category.keyword);
        }
        lines.push(LeftLine {
            plan: PlanCategory {
                keyword: category.keyword.clone(),
                name: category.name.clone(),
                slug: category.slug.clone(),
                // The keyword is the title's start; the merchant writes on.
                meta_title: category.name.clone(),
                out_of_ads: category.keep_out_of_ads(),
            },
            state,
        });
    }
    Ok(lines)
}

/// One place a row's form offers to send its keyword to.
struct Place {
    /// What the form posts: [`PARENT`], [`UNDER`] and a keyword, [`PARK`]
    /// or [`REMOVE`].
    to: String,
    label: String,
}

/// The places a keyword can go from where it stands: among the parents,
/// under each parent category but itself, parked, and out when it is in.
fn places(study: &StudyState, keyword: &str) -> Vec<Place> {
    let placed = study.tree.category(keyword);
    let parked = study.tree.is_parked(keyword);
    let has_children = study.tree.children_of(keyword).next().is_some();
    let mut places = Vec::new();
    if placed.is_none_or(|category| category.parent.is_some()) {
        places.push(Place {
            to: PARENT.to_owned(),
            label: "A parent category".to_owned(),
        });
    }
    // A parent category with children stays where it is until they move.
    if has_children {
        return Vec::new();
    }
    for parent in study.tree.parents() {
        let stands_there =
            placed.is_some_and(|category| category.parent.as_deref() == Some(&parent.keyword));
        if parent.keyword != keyword && !stands_there {
            places.push(Place {
                to: format!("{UNDER}{}", parent.keyword),
                label: format!("Under {}", timada_research::category_name(&parent.keyword)),
            });
        }
    }
    if !parked {
        places.push(Place {
            to: PARK.to_owned(),
            label: "Parked for later".to_owned(),
        });
    }
    if placed.is_some() || parked {
        places.push(Place {
            to: REMOVE.to_owned(),
            label: "Out of the tree".to_owned(),
        });
    }
    places
}

/// The form of a row: where its keyword goes.
#[component]
async fn place_form(
    cx: &Cx,
    action: &str,
    keyword: &str,
    list: &str,
    places: &[Place],
) -> Result<impl View> {
    Ok(view! {
        if !places.is_empty() {
            <form method="post" action=(action) class="flex justify-end gap-2" data-place="">
                <input type="hidden" name="keyword" value=(keyword)>
                <input type="hidden" name="list" value=(list)>
                select(
                    attrs: attributes! { name="to" aria-label=(format!("Where {keyword} goes")) class="w-48" },
                    for place in places {
                        <option value=(&place.to)>(&place.label)</option>
                    }
                )
                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" },
                    "Move"
                    <span class="sr-only">" " (keyword)</span>
                )
            </form>
        }
    })
}

/// A figure the source may have nothing for: never shown as a zero.
fn figure<T: ToString>(value: Option<T>) -> String {
    value.map_or_else(|| "–".to_owned(), |value| value.to_string())
}

/// A cost per click as the source gives it, in US dollars.
fn click(figures: Option<&KeywordFigures>) -> String {
    figures
        .and_then(|figures| figures.cpc_usd_cents)
        .map_or_else(
            || "–".to_owned(),
            |cents| format!("${}", timada_core::format_amount(cents)),
        )
}

/// What stands in a category's way, as the page words it.
fn problem_text(problem: &TreeProblem) -> String {
    match problem {
        TreeProblem::NotKept => "The study no longer keeps this keyword.".to_owned(),
        TreeProblem::NameTooLong => format!(
            "Its name is longer than the {MAX_CATEGORY_NAME_LEN} characters a category's may be."
        ),
        TreeProblem::NoSlug => "It gives no address a category can go by.".to_owned(),
        TreeProblem::SameSlug(other) => format!("It has the same address as « {other} »."),
        TreeProblem::NoNicheWord => "Its name holds none of the niche's words.".to_owned(),
    }
}

/// One category of the tree, as the page words it.
struct CategoryLine {
    keyword: String,
    name: String,
    slug: String,
    child: bool,
    volume: String,
    difficulty: String,
    cpc: String,
    /// Its own price as the form shows it; empty when it is the study's.
    own_price: String,
    /// The price its clicks are read against.
    price: String,
    ratio: String,
    /// `ads`, `keep_out`, or empty while it is not known.
    ads: &'static str,
    ads_label: &'static str,
    ads_tone: Tone,
    problems: Vec<(&'static str, String)>,
    places: Vec<Place>,
}

impl CategoryLine {
    fn of(category: &ReviewedCategory, study: &StudyState) -> Self {
        let currency = study.inputs.sale.currency.as_str();
        let (ads, ads_label, ads_tone) = match category.channel {
            Some(Channel::Ads) => ("ads", "Ads", Tone::Success),
            Some(Channel::SeoOnly) => ("keep_out", "Keep out of ads", Tone::Warning),
            None => ("", "Not known", Tone::Neutral),
        };
        Self {
            keyword: category.keyword.clone(),
            name: category.name.clone(),
            slug: category.slug.clone(),
            child: category.parent.is_some(),
            volume: figure(category.figures.and_then(|figures| figures.volume)),
            difficulty: figure(category.figures.and_then(|figures| figures.difficulty)),
            cpc: click(category.figures.as_ref()),
            own_price: category
                .own_price_cents
                .map(timada_core::format_amount)
                .unwrap_or_default(),
            price: category.price_cents.map_or_else(
                || "–".to_owned(),
                |cents| timada_core::format_money(cents, currency),
            ),
            ratio: category
                .price_per_click
                .map_or_else(|| "–".to_owned(), format_ratio),
            ads,
            ads_label,
            ads_tone,
            problems: category
                .problems
                .iter()
                .map(|problem| (problem.as_str(), problem_text(problem)))
                .collect(),
            places: places(study, &category.keyword),
        }
    }
}

/// One kept keyword, as the list words it.
struct KeptLine {
    keyword: String,
    volume: String,
    difficulty: String,
    cpc: String,
    /// `tree`, `parked`, or empty.
    stands: &'static str,
    places: Vec<Place>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let (study, user_id) = require_owned(cx).await?;
    let query = query_params::<TreeQuery>(cx)?;
    let db = context::<Db>(cx);
    let list = list_of(query.list.as_deref()).unwrap_or(LISTS[0].0);
    let after = query.after.as_deref().and_then(Cursor::parse);
    let before = query.before.as_deref().and_then(Cursor::parse);
    let listed = keywords_of_outcome(
        db,
        &study.id,
        list,
        PageFrom::of(after.as_ref(), before.as_ref()),
        PAGE_SIZE,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let counts = outcome_counts(db, &study.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let review = reviewed(cx, &study).await?;
    let categories: Vec<CategoryLine> = review
        .categories
        .iter()
        .map(|category| CategoryLine::of(category, &study))
        .collect();
    let parked: Vec<(String, Vec<Place>)> = study
        .tree
        .parked
        .iter()
        .map(|keyword| (keyword.clone(), places(&study, keyword)))
        .collect();
    let kept: Vec<KeptLine> = listed
        .rows
        .iter()
        .map(|row| KeptLine {
            keyword: row.keyword.clone(),
            volume: figure(row.volume),
            difficulty: figure(row.difficulty),
            cpc: row.cpc_usd_cents.map_or_else(
                || "–".to_owned(),
                |cents| format!("${}", timada_core::format_amount(cents as u64)),
            ),
            stands: if study.tree.category(&row.keyword).is_some() {
                "tree"
            } else if study.tree.is_parked(&row.keyword) {
                "parked"
            } else {
                ""
            },
            places: places(&study, &row.keyword),
        })
        .collect();
    let id = study.id.clone();
    let tabs: Vec<PageTab> = LISTS
        .iter()
        .map(|(key, label)| PageTab {
            label,
            href: format!("{}#kept", tree_url(cx, &id, Some(key), None, None)),
            count: Some(
                counts
                    .iter()
                    .find(|(outcome, _)| outcome == key)
                    .map_or(0, |(_, count)| *count as usize),
            ),
            current: *key == list,
        })
        .collect();
    let page_link = |after: Option<&str>, before: Option<&str>| {
        let url = href!(index, study_id: id.clone())
            .query(TreeLink {
                list: Some(list).filter(|list| *list != LISTS[0].0),
                shop: None,
                after,
                before,
                error: None,
                message: None,
            })
            .resolve(cx);
        format!("{url}#kept")
    };
    let more_searched = listed
        .newer
        .as_ref()
        .map(|cursor| page_link(None, Some(&cursor.param())));
    let less_searched = listed
        .older
        .as_ref()
        .map(|cursor| page_link(Some(&cursor.param()), None));
    let paged = more_searched.is_some() || less_searched.is_some();
    let currency = study.inputs.sale.currency.clone();
    let priced = !currency.is_empty();
    let held_back = review.held_back();
    let state = if review.categories.is_empty() {
        "empty"
    } else if held_back == 0 {
        "ready"
    } else {
        "held_back"
    };
    let (state_label, state_tone, state_text) = match state {
        "empty" => (
            "Empty",
            Tone::Neutral,
            "Place a kept keyword below to start the tree.".to_owned(),
        ),
        "ready" => (
            "Ready",
            Tone::Success,
            "Nothing stands in the way of any category.".to_owned(),
        ),
        _ => (
            "Held back",
            Tone::Danger,
            match held_back {
                1 => "1 category has something in its way.".to_owned(),
                n => format!("{n} categories have something in their way."),
            },
        ),
    };
    let many_parents = review.has_many_parents().then(|| {
        format!(
            "The tree has {} parent categories: {USUAL_PARENTS} at the most is the usual shape. A shop's menu reads at a glance with five or six.",
            review.parents
        )
    });
    let words_said = study.tree.niche_words.is_some();
    let form_words = review.niche_words.join("\n");
    let words_hint = if words_said {
        "One a line. A category's name must hold one of them, whatever its case, its accents and its number. Leave empty to check no name."
    } else {
        "One a line. Until you say, these are the words every seed keyword shares. A category's name must hold one of them. Leave empty to check no name."
    };
    let price_action = href!(price::price, study_id: id.clone()).resolve(cx);
    let place_action = href!(place::place, study_id: id.clone()).resolve(cx);
    let tree_description = "Each category is a keyword: its name and its address are read off it, and its figures are the keyword's. A parent category holds children; a child holds none. The price / CPC is the category's average price over its keyword's cost per click, and a category whose ratio is below what the study's sale needs is kept out of ads.";
    // What the study created, and what it would create: in its shop once
    // it has one, else in the shop looked at.
    let now = crate::auth::now();
    let linked = match &study.shop {
        Some(tenant_id) => tenancy(cx)
            .load_tenant(tenant_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?,
        None => None,
    };
    let looked_at = match (&linked, query.shop.as_deref()) {
        (None, Some(slug)) if !slug.is_empty() => manageable_shop(cx, &user_id, slug).await?,
        _ => None,
    };
    let target = linked.as_ref().or(looked_at.as_ref());
    let left = left_to_create(cx, &study, &review, target).await?;
    let creation = study.creation.as_ref();
    let pending = creation.is_some_and(|creation| creation.is_pending(now));
    let create_failed = creation.and_then(|creation| creation.failed.clone());
    let left_lines: Vec<(String, String, &'static str, &'static str, Option<String>)> = left
        .iter()
        .map(|line| {
            let (state, label) = match line.state {
                Left::Ready => ("ready", "Can be created"),
                Left::Taken => ("taken", "Its address is taken in this shop"),
                Left::UnderTaken => ("under_taken", "The category above it cannot be created"),
            };
            let refused = creation.and_then(|creation| {
                creation
                    .refused
                    .iter()
                    .find(|(keyword, _)| *keyword == line.plan.keyword)
                    .map(|(_, reason)| reason.clone())
            });
            (
                line.plan.name.clone(),
                line.plan.slug.clone(),
                state,
                label,
                refused,
            )
        })
        .collect();
    let creatable = left.iter().filter(|line| line.state == Left::Ready).count();
    let created_lines: Vec<(String, String, Option<String>)> = study
        .created
        .iter()
        .map(|created| {
            (
                timada_research::category_name(&created.keyword),
                created.slug.clone(),
                linked.as_ref().map(|tenant| {
                    crate::app::t::slug::categories::category_id::category_url(
                        cx,
                        &tenant.slug,
                        &created.category_id,
                        None,
                        None,
                    )
                }),
            )
        })
        .collect();
    let create_state = if pending {
        "pending"
    } else if study.shop.is_some() && left.is_empty() {
        "done"
    } else if study.shop.is_some() {
        "partial"
    } else {
        "none"
    };
    let shops: Vec<(String, String)> = if study.shop.is_none() {
        let mut shops = Vec::new();
        for row in tenants_of_user(db, &user_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        {
            if !row.suspended && manageable_shop(cx, &user_id, &row.slug).await?.is_some() {
                shops.push((row.slug, row.name));
            }
        }
        shops
    } else {
        Vec::new()
    };
    let target_name = target.map(|tenant| tenant.name.clone()).unwrap_or_default();
    let target_slug = target.map(|tenant| tenant.slug.clone()).unwrap_or_default();
    let linked_shop: Option<(String, String, String)> = linked.as_ref().map(|tenant| {
        (
            tenant.name.clone(),
            tenant.slug.clone(),
            href!(crate::app::t::slug::categories::index, crate::app::t::slug::slug: tenant.slug.clone())
                .resolve(cx),
        )
    });
    let has_target = target.is_some();
    let is_linked = linked.is_some();
    let create_label = match creatable {
        1 => "Create 1 draft category".to_owned(),
        n => format!("Create {n} draft categories"),
    };
    let create_action = href!(create::create, study_id: id.clone()).resolve(cx);
    let look_action = href!(index, study_id: id.clone()).resolve(cx);
    let ready = review.is_ready();
    let title = format!("Categories · {}", study.name);
    let subtitle = match (review.parents, review.categories.len() - review.parents) {
        (0, _) => "No category yet".to_owned(),
        (parents, children) => format!(
            "{parents} parent categor{}, {children} child{}",
            if parents == 1 { "y" } else { "ies" },
            if children == 1 { "" } else { "ren" }
        ),
    };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::page("research"),
            page_header(
                title: "Category tree",
                subtitle: Some(&subtitle),
                <a class=(LINK) href=(study_url(cx, &id, None, None)) data-back-to-study="">"Back to " (&study.name)</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if let Some(text) = &many_parents {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { class="mb-4" data-many-parents=(review.parents) },
                    icon(data: icons::WARNING)
                    alert_description((text))
                )
            }

            section_card(
                title: Some("The tree"),
                description: Some(tree_description),
                attrs: attributes! { id="tree" data-tree=(state) data-tree-categories=(categories.len()) },
                table(
                    table_header(
                        table_row(
                            table_head("Category")
                            table_head("Searches a month")
                            table_head("Difficulty")
                            table_head("Cost per click")
                            table_head("Average price")
                            table_head("Price / CPC")
                            table_head("Ads")
                            table_head(<span class="sr-only">"Move"</span>)
                        )
                    )
                    table_body(
                        if categories.is_empty() {
                            empty_row(colspan: 8, text: "No category yet: place a kept keyword below.")
                        }
                        for line in &categories {
                            table_row(
                                attrs: attributes! { data-category=(&line.keyword) data-category-level=(if line.child { "child" } else { "parent" }) data-category-slug=(&line.slug) },
                                table_cell(
                                    attrs: attributes! { class="whitespace-normal" },
                                    <div class=(if line.child { "pl-6" } else { "font-medium" })>
                                        <span data-category-name="">(&line.name)</span>
                                        <p class="text-xs font-normal text-muted-foreground">"/" (&line.slug)</p>
                                        for (key, text) in &line.problems {
                                            <p class="mt-1 text-sm font-normal text-destructive-text" data-category-problem=(*key)>(text)</p>
                                        }
                                    </div>
                                )
                                table_cell(attrs: attributes! { class="tabular-nums" data-volume="" }, (&line.volume))
                                table_cell(attrs: attributes! { class="tabular-nums" data-difficulty="" }, (&line.difficulty))
                                table_cell(attrs: attributes! { class="tabular-nums" data-cpc="" }, (&line.cpc))
                                table_cell(
                                    attrs: attributes! { class="tabular-nums" },
                                    <span data-category-price="">(&line.price)</span>
                                    if priced {
                                        <form method="post" action=(&price_action) class="mt-1 flex gap-2">
                                            <input type="hidden" name="keyword" value=(&line.keyword)>
                                            input(attrs: attributes! { name="price" type="text" inputmode="decimal" value=(&line.own_price) placeholder="Its own" aria-label=(format!("Average price of {}, in {currency}", line.name)) class="w-24" })
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" },
                                                "Save"
                                                <span class="sr-only">" the price of " (&line.name)</span>
                                            )
                                        </form>
                                    }
                                )
                                table_cell(attrs: attributes! { class="tabular-nums" data-ratio="" }, (&line.ratio))
                                table_cell(attrs: attributes! { data-category-ads=(line.ads) }, status_badge(tone: line.ads_tone, (line.ads_label)))
                                table_cell(
                                    place_form(action: &place_action, keyword: &line.keyword, list: list, places: &line.places)
                                    if line.places.is_empty() {
                                        <p class="text-right text-xs text-muted-foreground">"Move its children first"</p>
                                    }
                                )
                            )
                        }
                    )
                )
                if !priced {
                    <p class="px-6 text-sm text-muted-foreground" data-tree-unpriced="">
                        "Say the sale's price and its costs on the "
                        <a class=(LINK) href=(format!("{}#inputs", study_url(cx, &id, None, None)))>"study"</a>
                        " to give a category a price of its own and to know which to keep out of ads."
                    </p>
                }
            )

            section_card(
                title: Some("Checks"),
                description: Some("What is looked at before anything is created: every category has a keyword the study keeps, no two share a keyword or an address, and a name still holds the niche's own word: « gants moto », not « gants »."),
                attrs: attributes! { id="checks" },
                <p class="flex flex-wrap items-center gap-3 px-6 text-sm" data-tree-held-back=(held_back)>
                    status_badge(tone: state_tone, (state_label))
                    <span class="text-muted-foreground">(&state_text)</span>
                </p>
                <form method="post" action=(href!(words::words, study_id: id.clone())) class="grid gap-4 px-6 sm:max-w-md" data-niche-words=(if words_said { "said" } else { "seeds" })>
                    field(
                        field_label(attrs: attributes! { for="niche-words" }, "The niche's own words")
                        textarea(attrs: attributes! { id="niche-words" name="words" rows="3" aria-describedby="niche-words-hint" }, (&form_words))
                        field_description(attrs: attributes! { id="niche-words-hint" }, (words_hint))
                    )
                    <div>
                        button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Save the words")
                    </div>
                </form>
            )

            section_card(
                title: Some("In a shop"),
                description: Some("The tree is created as draft categories: each with its name and its address from its keyword, the keyword as the start of its title for search engines, and the « keep out of ads » mark. Nothing is published: a draft is shown to no shopper until you publish it in the shop."),
                attrs: attributes! { id="create" data-create=(create_state) data-create-created=(created_lines.len()) data-create-left=(left_lines.len()) },
                if pending {
                    <p class="px-6 text-sm text-muted-foreground" data-create-pending="">
                        "The categories are being created, which takes a moment. "
                        <a class=(LINK) href=(create_url(cx, &id, None, None, None))>"Look again"</a>
                    </p>
                }
                if let Some(reason) = &create_failed {
                    <p class="px-6 text-sm whitespace-normal text-destructive-text" data-create-failed="">(reason)</p>
                }
                if let Some((name, slug, link)) = &linked_shop {
                    <p class="px-6 text-sm" data-create-shop=(slug)>
                        "This study's shop is "
                        <a class=(LINK) href=(link)>(name)</a>
                        ": its categories are created there."
                    </p>
                }
                if !created_lines.is_empty() {
                    table(
                        table_header(
                            table_row(
                                table_head("Created")
                                table_head("Address")
                            )
                        )
                        table_body(
                            for (name, slug, link) in &created_lines {
                                table_row(
                                    attrs: attributes! { data-created=(slug) },
                                    table_cell(attrs: attributes! { class="font-medium" },
                                        if let Some(link) = link {
                                            <a class=(LINK) href=(link)>(name)</a>
                                        } else {
                                            (name)
                                        }
                                    )
                                    table_cell(<code class="text-xs">"/" (slug)</code>)
                                )
                            }
                        )
                    )
                }
                if !left_lines.is_empty() && has_target {
                    table(
                        table_header(
                            table_row(
                                table_head(if is_linked { "Left to create" } else { "Would be created" })
                                table_head("Address")
                                table_head("In this shop")
                            )
                        )
                        table_body(
                            for (name, slug, state, label, refused) in &left_lines {
                                table_row(
                                    attrs: attributes! { data-left=(slug) data-left-state=(*state) },
                                    table_cell(attrs: attributes! { class="font-medium" }, (name))
                                    table_cell(<code class="text-xs">"/" (slug)</code>)
                                    table_cell(
                                        attrs: attributes! { class="whitespace-normal" },
                                        (*label)
                                        if let Some(reason) = refused {
                                            <p class="mt-1 text-sm text-destructive-text" data-left-refused="">(reason)</p>
                                        }
                                    )
                                )
                            }
                        )
                    )
                }
                if !pending {
                    if !ready && !left_lines.is_empty() {
                        <p class="px-6 text-sm text-muted-foreground" data-create-held-back="">"Nothing is created while a category of the tree has something in its way: see the checks above."</p>
                    } else if has_target && creatable > 0 {
                        <form method="post" action=(&create_action) class="flex flex-wrap items-center gap-3 px-6">
                            <input type="hidden" name="shop" value=(&target_slug)>
                            button(attrs: attributes! { type="submit" data-create-in-shop=(creatable) }, (&create_label) " in " (&target_name))
                            if creatable < left_lines.len() {
                                <span class="text-sm text-muted-foreground">"The others stay left to create until their address is free."</span>
                            }
                        </form>
                    } else if has_target && !left_lines.is_empty() {
                        <p class="px-6 text-sm text-muted-foreground">"No category can be created in this shop as it stands: free an address there, or take its keyword out of the tree."</p>
                    }
                    if ready && !is_linked && !left_lines.is_empty() {
                        <div class="grid gap-6 px-6 sm:grid-cols-2">
                            if shops.is_empty() {
                                <div class="flex flex-col gap-2" data-create-pick="none">
                                    <p class="text-sm font-medium">"Create in a shop"</p>
                                    <p class="text-sm text-muted-foreground">"You have no shop whose catalog you may manage yet."</p>
                                </div>
                            } else {
                                <form method="get" action=(&look_action) class="flex flex-col gap-3" data-create-pick="">
                                    field(
                                        field_label(attrs: attributes! { for="create-shop" }, "Create in a shop")
                                        field_description(attrs: attributes! { id="create-shop-hint" }, "One whose catalog you may manage. You see what would be created, and which addresses are taken there, before anything is.")
                                        select(
                                            attrs: attributes! { id="create-shop" name="shop" aria-describedby="create-shop-hint" },
                                            for (slug, name) in &shops {
                                                <option value=(slug) selected=(*slug == target_slug)>(name)</option>
                                            }
                                        )
                                    )
                                    <div>button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "See what would be created")</div>
                                </form>
                            }
                            <form method="post" action=(&create_action) class="flex flex-col gap-3" data-create-new="">
                                field(
                                    field_label(attrs: attributes! { for="create-name" }, "Create a shop from this study")
                                    field_description(attrs: attributes! { id="create-name-hint" }, "The shop is created as the « new shop » page would, yours, then the tree in it. If a category fails, the shop stays and this page says what is left.")
                                    input(attrs: attributes! { id="create-name" name="name" required="" maxlength="80" value=(&study.name) aria-describedby="create-name-hint" })
                                )
                                <div>button(attrs: attributes! { type="submit" }, "Create the shop and its categories")</div>
                            </form>
                        </div>
                    }
                }
            )

            if !parked.is_empty() {
                section_card(
                    title: Some("Parked for later"),
                    description: Some("Keywords kept in sight, out of the tree."),
                    attrs: attributes! { id="parked" },
                    table(
                        table_header(
                            table_row(
                                table_head("Keyword")
                                table_head(<span class="sr-only">"Move"</span>)
                            )
                        )
                        table_body(
                            for (keyword, places) in &parked {
                                table_row(
                                    attrs: attributes! { data-parked=(keyword) },
                                    table_cell(attrs: attributes! { class="font-medium" }, (keyword))
                                    table_cell(place_form(action: &place_action, keyword: keyword, list: list, places: places))
                                )
                            }
                        )
                    )
                )
            }

            section_card(
                title: Some("Kept keywords"),
                description: Some("The keywords the score keeps, the most searched first. Only these make categories."),
                attrs: attributes! { id="kept" },
                <div class="px-6">
                    page_tabs(label: "Kept keywords", tabs: &tabs)
                </div>
                table(
                    table_header(
                        table_row(
                            table_head("Keyword")
                            table_head("Searches a month")
                            table_head("Difficulty")
                            table_head("Cost per click")
                            table_head("Where it stands")
                            table_head(<span class="sr-only">"Move"</span>)
                        )
                    )
                    table_body(
                        if kept.is_empty() {
                            empty_row(colspan: 6, text: "No keyword in this list.")
                        }
                        for row in &kept {
                            table_row(
                                attrs: attributes! { data-kept=(&row.keyword) data-kept-stands=(row.stands) },
                                table_cell(attrs: attributes! { class="font-medium" }, (&row.keyword))
                                table_cell(attrs: attributes! { class="tabular-nums" }, (&row.volume))
                                table_cell(attrs: attributes! { class="tabular-nums" }, (&row.difficulty))
                                table_cell(attrs: attributes! { class="tabular-nums" }, (&row.cpc))
                                table_cell(
                                    (match row.stands {
                                        "tree" => "In the tree",
                                        "parked" => "Parked",
                                        _ => "–",
                                    })
                                )
                                table_cell(place_form(action: &place_action, keyword: &row.keyword, list: list, places: &row.places))
                            )
                        }
                    )
                )
                if paged {
                    pagination(
                        attrs: attributes! { class="mt-4" },
                        pagination_content(
                            if let Some(more_searched) = more_searched {
                                pagination_item(pagination_previous(label: "More searched", attrs: attributes! { href=(more_searched) rel="prev" }))
                            }
                            if let Some(less_searched) = less_searched {
                                pagination_item(pagination_next(label: "Less searched", attrs: attributes! { href=(less_searched) rel="next" }))
                            }
                        )
                    )
                }
            )
        )
    })
}
