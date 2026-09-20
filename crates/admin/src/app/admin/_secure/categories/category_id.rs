//! `/{mount}/categories/{category_id}`: a category — its name, text, place in
//! the tree and rank — and archiving it. The address (slug) never changes.

use serde::Deserialize;
use timada_catalog::{
    CatalogError, CategoryRow, category_by_id, category_lineage, category_subtree_ids,
    count_products_in_categories,
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
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
        textarea::textarea,
    },
    config::AdminServices,
    ui::page_header,
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

    Ok(view! {
        page_header(
            title: &category.name,
            if category.archived { <span class="text-sm text-muted-foreground">"Archivée"</span> }
        )
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">
            (breadcrumb) " · " <span class="font-mono text-xs">"/c/" (category.slug.clone())</span>
            " · " (products.to_string()) " produit(s) en vente, sous-catégories comprises"
        </p>
        if let Some(error) = &error {
            <p role="alert" class="mb-4 text-sm text-destructive">(error.clone())</p>
        }
        <div class="grid gap-6 lg:grid-cols-3">
            <div class="lg:col-span-2">
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
            </div>
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
        </div>
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
            None => return Err(anyhow::Error::from(err).into()),
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
