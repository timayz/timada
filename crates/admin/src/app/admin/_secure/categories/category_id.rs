//! `/{mount}/categories/{category_id}`: a category — its name, text, place in
//! the tree and rank — and archiving it. The address (slug) never changes.

use serde::Deserialize;
use timada_catalog::{
    CatalogError, CategoryRow, SpecKey, category_by_id, category_lineage, category_subtree_ids,
    count_products_in_categories, effective_facets, specs_in_category,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param, query_params, query_params as query,
    },
    view::{View, view},
};

use super::{category_options, category_select, refusal};
use crate::{
    auth::Section,
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
        textarea::textarea,
    },
    config::AdminServices,
    ui::{detail_grid, detail_main, detail_side, form_error, page_header},
};

path_param!(pub category_id: String, error = not_found);

#[query_params(error = bad_request)]
struct ShowQuery {
    error: Option<String>,
}

async fn load(cx: &Cx) -> Result<CategoryRow> {
    let id = param::<CategoryId>(cx)?.clone();
    let db = &app_context::<AdminServices>(cx).db;
    Ok(category_by_id(db, &id).await?.ok_or_not_found()?)
}

fn back(cx: &Cx, id: &str) -> String {
    href!(show, CategoryId(id.to_owned())).resolve(cx)
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let category = load(cx).await?;
    let db = &app_context::<AdminServices>(cx).db;
    let error = query::<ShowQuery>(cx)?.error.clone();
    let lineage = category_lineage(db, &category.id).await?;
    let breadcrumb = lineage
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(" > ");
    let products =
        count_products_in_categories(db, &category_subtree_ids(db, &category.id).await?).await?;
    // Anywhere but under itself.
    let parents = category_options(cx, Some(&category.id)).await?;
    // The spec filters: the category's own list, what it inherits when it has
    // none, and the specs its products actually have, to pick from.
    let own_facets = facet_lines(&category.facet_keys());
    let inherited = if own_facets.is_empty() {
        facet_lines(&effective_facets(&lineage)).replace('\n', ", ")
    } else {
        String::new()
    };
    let available: Vec<(String, String)> = specs_in_category(db, &category.id)
        .await?
        .into_iter()
        .map(|(key, products)| (facet_lines(&[key]), products.to_string()))
        .collect();

    Ok(view! {
        page_header(
            parent: Section::Categories,
            title: &category.name,
            if category.archived { <span class="text-sm text-muted-foreground">"Archivée"</span> }
        )
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">
            (breadcrumb) " · " <span class="font-mono text-xs">"/c/" (category.slug.clone())</span>
            " · " (products.to_string()) " produit(s) en vente, sous-catégories comprises"
        </p>
        if let Some(error) = &error {
            form_error(class: "mb-4", (error.clone()))
        }
        detail_grid(
            detail_main(
                card(
                    card_header(card_title("Catégorie"))
                    card_content(
                        <form method="post" action=(href!(update, CategoryId(category.id.clone()))) class="flex flex-col gap-4">
                            <fieldset disabled=(category.archived) class="flex flex-col gap-4">
                                <div class="flex flex-col gap-1.5">
                                    label(attrs: topcoat::view::attributes! { for="name" }, "Nom")
                                    input(attrs: topcoat::view::attributes! { id="name" name="name" required=(true) value=(category.name.clone()) })
                                </div>
                                <div class="flex flex-col gap-1.5">
                                    label(attrs: topcoat::view::attributes! { for="description" }, "Texte en tête de la page de la catégorie")
                                    textarea(attrs: topcoat::view::attributes! { id="description" name="description" rows="4" }, (category.description.clone()))
                                </div>
                                <div class="grid gap-4 sm:grid-cols-2">
                                    <div class="flex flex-col gap-1.5">
                                        label(attrs: topcoat::view::attributes! { for="parent_id" }, "Rangée sous")
                                        category_select(name: "parent_id", options: &parents, selected: category.parent_id.as_deref(), none_label: Some("— la racine —"))
                                    </div>
                                    <div class="flex flex-col gap-1.5">
                                        label(attrs: topcoat::view::attributes! { for="position" }, "Rang parmi ses voisines (le plus petit d'abord)")
                                        input(attrs: topcoat::view::attributes! { id="position" name="position" type="number" min="0" value=(category.position.to_string()) })
                                    </div>
                                </div>
                                if !category.archived {
                                    <div>button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer")</div>
                                }
                            </fieldset>
                        </form>
                    )
                )
            )
            detail_side(
            if !category.archived {
                card(
                    card_header(card_title("Filtres de la fiche technique"))
                    card_content(
                        <form method="post" action=(href!(define_facets, CategoryId(category.id.clone()))) class="flex flex-col gap-3">
                            label(attrs: topcoat::view::attributes! { for="facets" }, "Un par ligne : Groupe > Libellé, dans l'ordre affiché")
                            textarea(attrs: topcoat::view::attributes! { id="facets" name="facets" rows="5" placeholder="Dalle > Taille" }, (own_facets.clone()))
                            if !inherited.is_empty() {
                                <p class="text-sm text-muted-foreground">"Sans liste propre, la catégorie hérite de : " (inherited.clone())</p>
                            }
                            <div>button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer les filtres")</div>
                        </form>
                        if !available.is_empty() {
                            <h3 class="mt-4 mb-2 text-sm font-medium">"Caractéristiques des produits de la catégorie"</h3>
                            <ul class="flex flex-col gap-1 text-sm text-muted-foreground">
                                for (line, products) in &available {
                                    <li><span class="font-mono text-xs">(line.clone())</span> " — " (products.clone()) " produit(s)"</li>
                                }
                            </ul>
                        }
                    )
                )
            }
            if !category.archived {
                card(
                    card_header(card_title("Archiver"))
                    card_content(
                        <p class="mb-4 text-sm text-muted-foreground">"La catégorie quitte la boutique avec tout ce qu'elle contient. Ses produits restent en vente et peuvent être rangés ailleurs. C'est définitif."</p>
                        <form method="post" action=(href!(archive, CategoryId(category.id.clone())))>
                            button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" }, "Archiver la catégorie")
                        </form>
                    )
                )
            }
            )
        )
    })
}

#[derive(Debug, Deserialize)]
pub struct UpdateForm {
    name: String,
    description: String,
    parent_id: String,
    position: u32,
}

/// Applies what changed — each command ignores what did not.
async fn apply(cx: &Cx, id: &str, form: UpdateForm) -> std::result::Result<(), CatalogError> {
    let services = app_context::<AdminServices>(cx);
    let cmd = timada_catalog::Command(&services.executor);
    cmd.rename_category(id, &form.name).await?;
    cmd.describe_category(id, &form.description).await?;
    cmd.position_category(id, form.position).await?;
    cmd.move_category(id, Some(form.parent_id).filter(|p| !p.is_empty()))
        .await
}

/// The list this page reads trails the commands by one subscription: a
/// reload right after saving may still show the previous values.
#[page(POST "./update")]
pub async fn update(cx: &Cx, Form(form): Form<UpdateForm>) -> Result<impl View> {
    let category = load(cx).await?;
    let target = match apply(cx, &category.id, form).await {
        Ok(()) => back(cx, &category.id),
        Err(err) => match refusal(&err) {
            Some(message) => href!(show, CategoryId(category.id.clone()))
                .query([("error", message)])
                .resolve(cx),
            None => return Err(err.into()),
        },
    };
    Err::<(), _>(see_other(target).into())
}

#[page(POST "./archive")]
pub async fn archive(cx: &Cx) -> Result<impl View> {
    let category = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_catalog::Command(&services.executor)
        .archive_category(&category.id)
        .await?;
    Err::<(), _>(see_other(back(cx, &category.id)).into())
}

/// `Groupe > Libellé`, a line per spec.
fn facet_lines(facets: &[SpecKey]) -> String {
    facets
        .iter()
        .map(|facet| format!("{} > {}", facet.group, facet.label))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Deserialize)]
pub struct FacetsForm {
    facets: String,
}

/// Replaces the category's list of spec filters; an empty box hands the
/// category back to its parent's list.
#[page(POST "./facets")]
pub async fn define_facets(cx: &Cx, Form(form): Form<FacetsForm>) -> Result<impl View> {
    let category = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let facets = form
        .facets
        .lines()
        .map(|line| match line.split_once('>') {
            Some((group, name)) => SpecKey::new(group, name),
            None => SpecKey::new("", line),
        })
        .collect();
    let defined = timada_catalog::Command(&services.executor)
        .define_category_facets(&category.id, facets)
        .await;
    let target = match defined {
        Ok(()) => back(cx, &category.id),
        Err(CatalogError::TooManyFacets(max)) => href!(show, CategoryId(category.id.clone()))
            .query([("error", format!("{max} filtres au plus par catégorie."))])
            .resolve(cx),
        Err(err) => match refusal(&err) {
            Some(message) => href!(show, CategoryId(category.id.clone()))
                .query([("error", message)])
                .resolve(cx),
            None => return Err(err.into()),
        },
    };
    Err::<(), _>(see_other(target).into())
}
