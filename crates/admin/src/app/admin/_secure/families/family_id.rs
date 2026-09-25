//! `/{mount}/families/{family_id}`: a family — its name, what tells its
//! variants apart, and the products standing in it. Read from the events, not
//! from the list: what was just saved shows at once.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::{CatalogError, FamilyOption, FamilyState, OptionValue, products_by_ids};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param, query_params, query_params as query,
    },
    view::{View, view},
};

use super::{option_lines, refusal};
use crate::{
    app::admin::_secure::products::product_id::{ProductId, show as show_product},
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
        select::select,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
        textarea::textarea,
    },
    config::AdminServices,
    ui::{detail_grid, detail_main, form_error, link, page_header},
};

path_param!(pub family_id: String, error = not_found);

#[query_params(error = bad_request)]
struct ShowQuery {
    error: Option<String>,
}

async fn load(cx: &Cx) -> Result<FamilyState> {
    let id = param::<FamilyId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    Ok(timada_catalog::Command(&services.executor)
        .load_family(id)
        .await?
        .ok_or_not_found()?)
}

/// Back to the family, with what the catalog refused, if it did.
fn back(cx: &Cx, id: &str, outcome: std::result::Result<(), CatalogError>) -> Result<String> {
    let page = href!(show, FamilyId(id.to_owned()));
    match outcome {
        Ok(()) => Ok(page.resolve(cx)),
        Err(err) => match refusal(&err) {
            Some(message) => Ok(page.query([("error", message)]).resolve(cx)),
            None => Err(anyhow::Error::from(err).into()),
        },
    }
}

/// A variant as the table shows it.
struct Line {
    product_id: String,
    link: String,
    name: String,
    sku: String,
    archived: bool,
    /// One cell per option, in the options' order; empty when not said.
    places: Vec<String>,
    complete: bool,
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let family = load(cx).await?;
    let db = &app_context::<AdminServices>(cx).db;
    let error = query::<ShowQuery>(cx)?.error.clone();
    let ids: Vec<String> = family
        .variants
        .iter()
        .map(|variant| variant.product_id.clone())
        .collect();
    let products: HashMap<String, _> = products_by_ids(db, &ids)
        .await?
        .into_iter()
        .map(|product| (product.id.clone(), product))
        .collect();
    let lines: Vec<Line> = family
        .variants
        .iter()
        .map(|variant| {
            let product = products.get(&variant.product_id);
            Line {
                product_id: variant.product_id.clone(),
                link: href!(show_product, ProductId(variant.product_id.clone())).resolve(cx),
                name: product.map_or_else(|| variant.product_id.clone(), |p| p.name.clone()),
                sku: product.map(|p| p.sku.clone()).unwrap_or_default(),
                archived: product.is_some_and(|p| p.archived),
                places: family
                    .options
                    .iter()
                    .map(|option| {
                        variant
                            .value_of(&option.name)
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .collect(),
                complete: family.is_complete(variant),
            }
        })
        .collect();
    let options_text = option_lines(&family.options);
    let selects: Vec<(String, String, Vec<String>)> = family
        .options
        .iter()
        .enumerate()
        .map(|(n, option)| {
            (
                format!("value_{n}"),
                option.name.clone(),
                option.values.clone(),
            )
        })
        .collect();
    let open = !family.dissolved;

    Ok(view! {
        page_header(
            title: &family.name,
            if family.dissolved { <span class="text-sm text-muted-foreground">"Dissoute"</span> }
        )
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">
            <span class="font-mono text-xs">(family.slug.clone())</span>
            " · " (lines.len().to_string()) " variante(s) — chacune reste un produit, avec sa référence, son prix, son stock et sa page"
        </p>
        if let Some(error) = &error {
            form_error(class: "mb-4", (error.clone()))
        }
        detail_grid(
            detail_main(
                card(
                    card_header(card_title("Variantes"))
                    card_content(
                        if lines.is_empty() {
                            <p class="text-sm text-muted-foreground">"Aucune variante pour l'instant."</p>
                        } else {
                            table(
                                table_header(table_row(
                                    table_head("Produit")
                                    for option in &family.options { table_head((option.name.clone())) }
                                    table_head("")
                                ))
                                table_body(
                                    for line in &lines {
                                        table_row(
                                            table_cell(
                                                link(href: line.link.clone(), (line.name.clone()))
                                                <span class="ml-2 font-mono text-xs text-muted-foreground">(line.sku.clone())</span>
                                                if line.archived { <span class="ml-2 text-xs text-muted-foreground">"archivé"</span> }
                                                if !line.complete { <span class="ml-2 text-xs text-destructive">"à compléter"</span> }
                                            )
                                            for cell in &line.places { table_cell((cell.clone())) }
                                            table_cell(
                                                <form method="post" action=(href!(remove, FamilyId(family.id.clone())))>
                                                    <input type="hidden" name="product_id" value=(line.product_id.clone())>
                                                    button(variant: ButtonVariant::Ghost, attrs: topcoat::view::attributes! { type="submit" }, "Retirer")
                                                </form>
                                            )
                                        )
                                    }
                                )
                            )
                        }
                        if open && !selects.is_empty() {
                            <form method="post" action=(href!(place, FamilyId(family.id.clone()))) class="mt-6 grid gap-4 sm:grid-cols-2">
                                <div class="flex flex-col gap-1.5 sm:col-span-2">
                                    label(attrs: topcoat::view::attributes! { for="sku" }, "Référence (SKU) du produit — celle d'une variante déjà placée la déplace")
                                    input(attrs: topcoat::view::attributes! { id="sku" name="sku" required=(true) })
                                </div>
                                for (field, option_name, values) in &selects {
                                    <div class="flex flex-col gap-1.5">
                                        label(attrs: topcoat::view::attributes! { for=(field.clone()) }, (option_name.clone()))
                                        select(
                                            attrs: topcoat::view::attributes! { id=(field.clone()) name=(field.clone()) required=(true) class="w-full" },
                                            <option value="">"— choisir —"</option>
                                            for value in values { <option value=(value.clone())>(value.clone())</option> }
                                        )
                                    </div>
                                }
                                <div class="sm:col-span-2">button(attrs: topcoat::view::attributes! { type="submit" }, "Placer dans la famille")</div>
                            </form>
                        }
                        if open && selects.is_empty() {
                            <p class="mt-4 text-sm text-muted-foreground">"Dites d'abord ce qui distingue les variantes."</p>
                        }
                    )
                )
            )
            detail_main(
                if open {
                    card(
                        card_header(card_title("Famille"))
                        card_content(
                            <form method="post" action=(href!(rename, FamilyId(family.id.clone()))) class="flex flex-col gap-3">
                                label(attrs: topcoat::view::attributes! { for="name" }, "Nom")
                                input(attrs: topcoat::view::attributes! { id="name" name="name" required=(true) value=(family.name.clone()) })
                                <div>button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Renommer")</div>
                            </form>
                        )
                    )
                    card(
                        card_header(card_title("Ce qui distingue les variantes"))
                        card_content(
                            <form method="post" action=(href!(define_options, FamilyId(family.id.clone()))) class="flex flex-col gap-3">
                                label(attrs: topcoat::view::attributes! { for="options" }, "Une option par ligne, ses valeurs dans l'ordre proposé aux clients")
                                textarea(attrs: topcoat::view::attributes! { id="options" name="options" rows="4" placeholder="Couleur : Noir, Argent\nCapacité : 64 Go, 128 Go" }, (options_text.clone()))
                                <p class="text-sm text-muted-foreground">"Une valeur occupée par une variante ne se retire pas."</p>
                                <div>button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer les options")</div>
                            </form>
                        )
                    )
                    card(
                        card_header(card_title("Dissoudre"))
                        card_content(
                            <p class="mb-4 text-sm text-muted-foreground">"Une famille ne se dissout que vide. Ses produits, eux, ne changent pas. C'est définitif."</p>
                            <form method="post" action=(href!(dissolve, FamilyId(family.id.clone())))>
                                button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" }, "Dissoudre la famille")
                            </form>
                        )
                    )
                }
            )
        )
    })
}

#[derive(Debug, Deserialize)]
pub struct RenameForm {
    name: String,
}

#[page(POST "./rename")]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<impl View> {
    let family = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let renamed = timada_catalog::Command(&services.executor)
        .rename_family(&family.id, &form.name)
        .await;
    Err::<(), _>(see_other(back(cx, &family.id, renamed)?).into())
}

#[derive(Debug, Deserialize)]
pub struct OptionsForm {
    options: String,
}

/// `Couleur : Noir, Argent` per line; a line without values names an option
/// that offers nothing yet.
#[page(POST "./options")]
pub async fn define_options(cx: &Cx, Form(form): Form<OptionsForm>) -> Result<impl View> {
    let family = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let options = form
        .options
        .lines()
        .map(|line| {
            let (name, values) = line.split_once(':').unwrap_or((line, ""));
            FamilyOption {
                name: name.to_owned(),
                values: values.split(',').map(str::to_owned).collect(),
            }
        })
        .collect();
    let defined = timada_catalog::Command(&services.executor)
        .define_family_options(&family.id, options)
        .await;
    Err::<(), _>(see_other(back(cx, &family.id, defined)?).into())
}

/// The product by its reference, and `value_<n>` for the family's n-th option.
#[page(POST "./place")]
pub async fn place(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<impl View> {
    let family = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let sku = form.get("sku").map(|sku| sku.trim()).unwrap_or_default();
    let values: Vec<OptionValue> = family
        .options
        .iter()
        .enumerate()
        .filter_map(|(n, option)| {
            let value = form.get(&format!("value_{n}"))?;
            Some(OptionValue::new(&option.name, value))
        })
        .collect();
    let placed = timada_catalog::Command(&services.executor)
        .place_variant(&family.id, timada_catalog::product_id(sku), values)
        .await
        .map(|_| ());
    Err::<(), _>(see_other(back(cx, &family.id, placed)?).into())
}

#[derive(Debug, Deserialize)]
pub struct RemoveForm {
    product_id: String,
}

#[page(POST "./remove")]
pub async fn remove(cx: &Cx, Form(form): Form<RemoveForm>) -> Result<impl View> {
    let family = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let removed = timada_catalog::Command(&services.executor)
        .remove_variant(&family.id, &form.product_id)
        .await
        .map(|_| ());
    Err::<(), _>(see_other(back(cx, &family.id, removed)?).into())
}

#[page(POST "./dissolve")]
pub async fn dissolve(cx: &Cx) -> Result<impl View> {
    let family = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let dissolved = timada_catalog::Command(&services.executor)
        .dissolve_family(&family.id)
        .await;
    Err::<(), _>(see_other(back(cx, &family.id, dissolved)?).into())
}
