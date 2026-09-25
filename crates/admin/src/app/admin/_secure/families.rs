//! `/{mount}/families`: the product families — articles sold in several
//! versions, each version a product of its own — and opening one.

pub mod family_id;

use serde::Deserialize;
use timada_catalog::{CatalogError, CreateFamily, FamilyOption, list_families};
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
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{detail_grid, detail_main, empty_state, form_error, link, page_header, table_card},
};

/// What the operator is told when the catalog refuses.
pub fn refusal(err: &CatalogError) -> Option<String> {
    Some(match err {
        CatalogError::FamilySlugAlreadyExists(slug) => {
            format!("Le mot « {slug} » est déjà celui d'une autre famille.")
        }
        CatalogError::InvalidSlug(_) => {
            "Le mot ne contient que des minuscules, des chiffres et des tirets.".to_owned()
        }
        CatalogError::Required("name") => "Le nom est obligatoire.".to_owned(),
        CatalogError::Required("options") => {
            "Dites d'abord ce qui distingue les variantes : la famille n'a aucune option."
                .to_owned()
        }
        CatalogError::Required(_) => "Ce nom ne donne aucun mot : saisissez-en un.".to_owned(),
        CatalogError::FamilyNotFound => "Cette famille n'existe pas.".to_owned(),
        CatalogError::FamilyDissolved => "Cette famille est dissoute.".to_owned(),
        CatalogError::FamilyNotEmpty => {
            "Retirez d'abord les variantes : une famille ne se dissout que vide.".to_owned()
        }
        CatalogError::TooManyOptions(max) => format!("{max} options au plus par famille."),
        CatalogError::TooManyOptionValues(max) => format!("{max} valeurs au plus par option."),
        CatalogError::DuplicateOption(name) => format!("L'option « {name} » est nommée deux fois."),
        CatalogError::OptionInUse { option, value } => {
            format!("« {option} : {value} » est la place d'une variante : déplacez-la d'abord.")
        }
        CatalogError::MissingOptionValue(option) => {
            format!("Choisissez une valeur pour « {option} ».")
        }
        CatalogError::UnknownOptionValue { option, value } => {
            format!("« {value} » n'est pas une valeur de « {option} ».")
        }
        CatalogError::VariantPlaceTaken => "Une autre variante occupe déjà cette place.".to_owned(),
        CatalogError::ProductInAnotherFamily => {
            "Ce produit est la variante d'une autre famille : retirez-le d'abord de celle-ci."
                .to_owned()
        }
        CatalogError::ProductNotFound => "Aucun produit ne porte cette référence.".to_owned(),
        CatalogError::ProductArchived => "Ce produit est archivé.".to_owned(),
        _ => return None,
    })
}

/// `Couleur : Noir, Argent` — a line per option, as the operator types them.
pub fn option_lines(options: &[FamilyOption]) -> String {
    options
        .iter()
        .map(|option| format!("{} : {}", option.name, option.values.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

struct Line {
    link: String,
    name: String,
    told_apart_by: String,
    variants: i64,
    dissolved: bool,
}

#[page]
pub async fn index() -> Result<impl View> {
    Ok(view! { families_view(error: None) })
}

#[derive(Debug, Deserialize)]
pub struct NewFamilyForm {
    name: String,
    slug: String,
}

#[page(POST "./new")]
pub async fn create(cx: &Cx, Form(form): Form<NewFamilyForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let created = timada_catalog::Command(&services.executor)
        .create_family(CreateFamily {
            name: form.name,
            slug: Some(form.slug),
        })
        .await;
    let error = match created {
        Ok(id) => {
            let show = href!(family_id::show, family_id::FamilyId(id)).resolve(cx);
            return Err(see_other(show).into());
        }
        Err(err) => match refusal(&err) {
            Some(message) => message,
            None => return Err(anyhow::Error::from(err).into()),
        },
    };
    Ok(view! { families_view(error: Some(error)) })
}

#[component]
async fn families_view(cx: &Cx, error: Option<String>) -> Result<impl View> {
    let db = &app_context::<AdminServices>(cx).db;
    let lines: Vec<Line> = list_families(db, true)
        .await?
        .into_iter()
        .map(|family| Line {
            link: href!(family_id::show, family_id::FamilyId(family.id.clone())).resolve(cx),
            told_apart_by: family
                .option_list()
                .iter()
                .map(|option| option.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            name: family.name,
            variants: family.variant_count,
            dissolved: family.dissolved,
        })
        .collect();

    Ok(view! {
        page_header(title: "Familles de produits")
        detail_grid(
            detail_main(
                if lines.is_empty() {
                    empty_state(message: "Aucune famille. Une famille réunit les produits qui sont un même article en plusieurs versions : couleurs, capacités, tailles.")
                } else {
                    table_card(
                        table(
                            table_header(table_row(
                                table_head("Famille") table_head("Distinguée par")
                                table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Variantes")
                            ))
                            table_body(
                                for line in &lines {
                                    table_row(
                                        table_cell(
                                            link(href: line.link.clone(), (line.name.clone()))
                                            if line.dissolved { <span class="ml-2 text-xs text-muted-foreground">"dissoute"</span> }
                                        )
                                        table_cell(<span class="text-muted-foreground">(line.told_apart_by.clone())</span>)
                                        table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (line.variants.to_string()))
                                    )
                                }
                            )
                        )
                    )
                }
            )
            card(
                card_header(card_title("Nouvelle famille"))
                card_content(
                    <form method="post" action=(href!(create).resolve(cx)) class="flex flex-col gap-4">
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="name" }, "Nom")
                            input(attrs: topcoat::view::attributes! { id="name" name="name" required=(true) placeholder="Baladeur NW-A" })
                        </div>
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="slug" }, "Mot (définitif ; déduit du nom si vide)")
                            input(attrs: topcoat::view::attributes! { id="slug" name="slug" placeholder="baladeur-nw-a" pattern="[a-z0-9]+(-[a-z0-9]+)*" })
                        </div>
                        if let Some(error) = &error {
                            form_error((error.clone()))
                        }
                        <div>button(attrs: topcoat::view::attributes! { type="submit" }, "Ouvrir la famille")</div>
                    </form>
                )
            )
        )
    })
}
