//! `/{mount}/categories`: the category tree products are filed under — the
//! whole tree, archived branches included, and opening a category.

pub mod category_id;

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::{
    CatalogError, CategoryNode, CreateCategory, category_tree, list_categories,
    product_counts_by_category,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page},
    view::{View, component, view},
};

use crate::{
    components::{
        button::button,
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
        select::select,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{detail_grid, detail_main, empty_state, form_error, link, page_header, table_card},
};

/// One `<option>` of a category picker: the id and the name, indented by depth.
pub type CategoryOption = (String, String);

/// Every open category as picker options, the tree flattened depth first.
/// `without` leaves a branch out — a category cannot go under itself.
pub async fn category_options(cx: &Cx, without: Option<&str>) -> Result<Vec<CategoryOption>> {
    let db = &app_context::<AdminServices>(cx).db;
    let tree = category_tree(list_categories(db, false).await?, false);
    fn collect(
        nodes: &[CategoryNode],
        depth: usize,
        without: Option<&str>,
        into: &mut Vec<CategoryOption>,
    ) {
        for node in nodes {
            if Some(node.category.id.as_str()) == without {
                continue;
            }
            let indented = format!("{}{}", "— ".repeat(depth), node.category.name);
            into.push((node.category.id.clone(), indented));
            collect(&node.children, depth + 1, without, into);
        }
    }
    let mut options = Vec::new();
    collect(&tree, 0, without, &mut options);
    Ok(options)
}

/// A `<select>` over the category tree; `none_label`, when given, is the
/// empty choice (the root, or "no category").
#[component]
pub async fn category_select(
    name: &str,
    options: &[CategoryOption],
    selected: Option<&str>,
    #[default] none_label: Option<&str>,
) -> Result<impl View> {
    Ok(view! {
        select(
            attrs: topcoat::view::attributes! { id=(name) name=(name) class="w-full" },
            if let Some(none_label) = none_label {
                <option value="" selected=(selected.is_none())>(none_label)</option>
            }
            for (id, option_label) in options {
                <option value=(id.clone()) selected=(selected == Some(id.as_str()))>(option_label.clone())</option>
            }
        )
    })
}

/// What the operator is told when the catalog refuses.
pub fn refusal(err: &CatalogError) -> Option<String> {
    Some(match err {
        CatalogError::SlugAlreadyExists(slug) => {
            format!("L'adresse « {slug} » est déjà celle d'une autre catégorie.")
        }
        CatalogError::InvalidSlug(_) => {
            "L'adresse ne contient que des minuscules, des chiffres et des tirets.".to_owned()
        }
        CatalogError::Required("name") => "Le nom est obligatoire.".to_owned(),
        CatalogError::Required(_) => {
            "Ce nom ne donne aucune adresse : saisissez-en une.".to_owned()
        }
        CatalogError::CategoryCycle => {
            "Une catégorie ne peut pas être rangée sous elle-même.".to_owned()
        }
        CatalogError::CategoryTooDeep(levels) => {
            format!("L'arborescence compte {levels} niveaux au plus.")
        }
        CatalogError::CategoryArchived => "Cette catégorie est archivée.".to_owned(),
        CatalogError::CategoryNotFound => "Cette catégorie n'existe pas.".to_owned(),
        _ => return None,
    })
}

/// A row of the tree as the page lists it.
struct Line {
    link: String,
    name: String,
    slug: String,
    depth: usize,
    archived: bool,
    products: i64,
}

#[page]
pub async fn index() -> Result<impl View> {
    Ok(view! { categories_view(error: None) })
}

#[derive(Debug, Deserialize)]
pub struct NewCategoryForm {
    name: String,
    slug: String,
    parent_id: String,
}

#[page(POST "./new")]
pub async fn create(cx: &Cx, Form(form): Form<NewCategoryForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let created = timada_catalog::Command(&services.executor)
        .create_category(CreateCategory {
            name: form.name,
            slug: Some(form.slug),
            parent_id: Some(form.parent_id).filter(|id| !id.is_empty()),
        })
        .await;
    let error = match created {
        Ok(id) => {
            let show = href!(category_id::show, category_id::CategoryId(id)).resolve(cx);
            return Err(see_other(show).into());
        }
        Err(err) => match refusal(&err) {
            Some(message) => message,
            None => return Err(err.into()),
        },
    };
    Ok(view! { categories_view(error: Some(error)) })
}

#[component]
async fn categories_view(cx: &Cx, error: Option<String>) -> Result<impl View> {
    let db = &app_context::<AdminServices>(cx).db;
    let counts: HashMap<String, i64> = product_counts_by_category(db).await?;
    let tree = category_tree(list_categories(db, true).await?, true);
    let lines: Vec<Line> = tree
        .iter()
        .flat_map(|node| node.flatten(0))
        .map(|(depth, category)| Line {
            link: href!(
                category_id::show,
                category_id::CategoryId(category.id.clone())
            )
            .resolve(cx),
            name: category.name.clone(),
            slug: category.slug.clone(),
            depth,
            archived: category.archived,
            products: counts.get(&category.id).copied().unwrap_or(0),
        })
        .collect();
    let parents = category_options(cx, None).await?;

    Ok(view! {
        page_header(title: "Catégories")
        detail_grid(
            detail_main(
                if lines.is_empty() {
                    empty_state(message: "Aucune catégorie. Ouvrez la première : les produits s'y rangent depuis leur fiche.")
                } else {
                    table_card(
                        table(
                            table_header(table_row(
                                table_head("Catégorie") table_head("Adresse")
                                table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Produits")
                            ))
                            table_body(
                                for line in &lines {
                                    table_row(
                                        table_cell(
                                            <span style=(format!("padding-left:{}rem", line.depth as f32 * 1.25))>
                                                link(href: line.link.clone(), (line.name.clone()))
                                                if line.archived { <span class="ml-2 text-xs text-muted-foreground">"archivée"</span> }
                                            </span>
                                        )
                                        table_cell(<span class="font-mono text-xs text-muted-foreground">(line.slug.clone())</span>)
                                        table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (line.products.to_string()))
                                    )
                                }
                            )
                        )
                    )
                }
            )
            card(
                card_header(card_title("Nouvelle catégorie"))
                card_content(
                    <form method="post" action=(href!(create).resolve(cx)) class="flex flex-col gap-4">
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="name" }, "Nom")
                            input(attrs: topcoat::view::attributes! { id="name" name="name" required=(true) })
                        </div>
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="slug" }, "Adresse (définitive ; déduite du nom si vide)")
                            input(attrs: topcoat::view::attributes! { id="slug" name="slug" placeholder="ecrans-pc" pattern="[a-z0-9]+(-[a-z0-9]+)*" })
                        </div>
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="parent_id" }, "Rangée sous")
                            category_select(name: "parent_id", options: &parents, selected: None, none_label: Some("— la racine —"))
                        </div>
                        if let Some(error) = &error {
                            form_error((error.clone()))
                        }
                        <div>button(attrs: topcoat::view::attributes! { type="submit" }, "Ouvrir la catégorie")</div>
                    </form>
                )
            )
        )
    })
}
