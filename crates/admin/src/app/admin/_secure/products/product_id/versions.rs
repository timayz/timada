//! `/{mount}/products/{product_id}/versions`: the versions a product is sold
//! in — colours, capacities, sizes — kept from the product's own page. The
//! operator declines the product, adds a version (a product of its own,
//! born from this one), moves or removes any of them, all here; the family
//! that gathers them in the catalog is never named.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::{
    CatalogError, CreateFamily, CreateProduct, DescribeProduct, FamilyOption, FamilyState,
    OptionValue, ProductPageView, products_by_ids,
};
use timada_pricing::{ListPrice, load_product_price, price_id};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page},
    view::{View, component, view},
};

use super::{ProductId, load, show};
use crate::{
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
    ui::link,
};

/// What the operator is told when the catalog refuses.
pub fn refusal(err: &CatalogError) -> Option<String> {
    Some(match err {
        CatalogError::FamilySlugAlreadyExists(_) => {
            "Un autre article porte déjà ce nom commun : donnez-en un autre.".to_owned()
        }
        CatalogError::Required("name") => "Le nom est obligatoire.".to_owned(),
        CatalogError::Required("sku") => "La référence est obligatoire.".to_owned(),
        CatalogError::Required("options") => {
            "Dites d'abord ce qui distingue les versions.".to_owned()
        }
        CatalogError::Required(_) => "Ce nom ne donne aucun mot : saisissez-en un.".to_owned(),
        CatalogError::SkuAlreadyExists(sku) => {
            format!("La référence « {sku} » est déjà celle d'un autre produit.")
        }
        CatalogError::FamilyNotFound | CatalogError::FamilyDissolved => {
            "Ce produit n'est plus décliné.".to_owned()
        }
        CatalogError::TooManyOptions(max) => format!("{max} options au plus."),
        CatalogError::TooManyOptionValues(max) => format!("{max} valeurs au plus par option."),
        CatalogError::DuplicateOption(name) => format!("L'option « {name} » est nommée deux fois."),
        CatalogError::OptionInUse { option, value } => {
            format!("« {option} : {value} » est la place d'une version : déplacez-la d'abord.")
        }
        CatalogError::MissingOptionValue(option) => {
            format!("Choisissez une valeur pour « {option} ».")
        }
        CatalogError::UnknownOptionValue { option, value } => {
            format!("« {value} » n'est pas une valeur de « {option} ».")
        }
        CatalogError::VariantPlaceTaken => "Une autre version occupe déjà cette place.".to_owned(),
        CatalogError::ProductInAnotherFamily => {
            "Ce produit est déjà une version d'un autre article.".to_owned()
        }
        CatalogError::ProductNotFound => "Ce produit n'existe pas.".to_owned(),
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

/// The reverse: `Couleur : Noir, Argent` per line; a line without values
/// names an option that offers nothing yet.
fn parse_options(text: &str) -> Vec<FamilyOption> {
    text.lines()
        .map(|line| {
            let (name, values) = line.split_once(':').unwrap_or((line, ""));
            FamilyOption {
                name: name.to_owned(),
                values: values.split(',').map(str::to_owned).collect(),
            }
        })
        .collect()
}

/// `value_<n>` for the family's n-th option, as the forms post it.
fn values_from(family: &FamilyState, form: &HashMap<String, String>) -> Vec<OptionValue> {
    family
        .options
        .iter()
        .enumerate()
        .filter_map(|(n, option)| {
            let value = form.get(&format!("value_{n}"))?;
            Some(OptionValue::new(&option.name, value))
        })
        .collect()
}

/// Back to the product, with what the catalog refused, if it did.
fn back(cx: &Cx, id: &str, outcome: std::result::Result<(), CatalogError>) -> Result<String> {
    let product = href!(show, ProductId(id.to_owned()));
    match outcome {
        Ok(()) => Ok(product.resolve(cx)),
        Err(err) => match refusal(&err) {
            Some(message) => Ok(product.query([("error", message)]).resolve(cx)),
            None => Err(err.into()),
        },
    }
}

/// A version as the table shows it.
struct Line {
    product_id: String,
    link: String,
    name: String,
    sku: String,
    archived: bool,
    /// This product's own row.
    here: bool,
    /// Where it stands on each option, in the options' order.
    choices: Vec<Choice>,
    complete: bool,
}

/// `value_<n>` for the n-th option: its name, what it offers, and where a
/// version stands on it.
struct Choice {
    field: String,
    option: String,
    values: Vec<String>,
    current: Option<String>,
}

fn choices_of(family: &FamilyState, product_id: Option<&str>) -> Vec<Choice> {
    let here = product_id.and_then(|id| family.variant(id));
    family
        .options
        .iter()
        .enumerate()
        .map(|(n, option)| Choice {
            field: format!("value_{n}"),
            option: option.name.clone(),
            values: option.values.clone(),
            current: here
                .and_then(|variant| variant.value_of(&option.name))
                .map(str::to_owned),
        })
        .collect()
}

/// The card: how the product is declined and every version it is sold in;
/// `family` is what the catalog knows of it, none while the product is sold
/// in one version only.
#[component]
pub async fn versions_card(
    cx: &Cx,
    product: &ProductPageView,
    family: Option<&FamilyState>,
) -> Result<impl View> {
    let db = &app_context::<AdminServices>(cx).db;
    let product_id = product.id.as_str();
    let ids: Vec<String> = family
        .map(|family| {
            family
                .variants
                .iter()
                .map(|variant| variant.product_id.clone())
                .collect()
        })
        .unwrap_or_default();
    let known: HashMap<String, _> = products_by_ids(db, &ids)
        .await?
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect();
    let lines: Vec<Line> = family
        .map(|family| {
            family
                .variants
                .iter()
                .map(|variant| {
                    let row = known.get(&variant.product_id);
                    Line {
                        product_id: variant.product_id.clone(),
                        link: href!(show, ProductId(variant.product_id.clone())).resolve(cx),
                        name: row.map_or_else(|| variant.product_id.clone(), |p| p.name.clone()),
                        sku: row.map(|p| p.sku.clone()).unwrap_or_default(),
                        archived: row.is_some_and(|p| p.archived),
                        here: variant.product_id == product_id,
                        choices: choices_of(family, Some(&variant.product_id)),
                        complete: family.is_complete(variant),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    // A product that says it is declined but has no place yet stands first,
    // to be placed.
    let unplaced = family.is_some_and(|family| family.variant(product_id).is_none());
    let own = family
        .map(|family| choices_of(family, None))
        .unwrap_or_default();
    let declined = family.is_some();
    let common_name = family.map_or(product.name.as_str(), |family| family.name.as_str());
    let options_text = family
        .map(|family| option_lines(&family.options))
        .unwrap_or_default();
    let told_apart = family.is_some_and(|family| !family.options.is_empty());

    Ok(view! {
        card(
            card_header(card_title("Versions"))
            card_content(
                if !declined {
                    <p class="mb-4 text-sm text-muted-foreground">
                        "Ce produit n'existe qu'en une version. Déclinez-le pour le vendre en plusieurs — couleurs, capacités, tailles —, chacune un produit avec sa référence, son prix et son stock, réglés depuis sa page."
                    </p>
                }
                <form method="post" action=(href!(define, ProductId(product_id.to_owned()))) class="flex flex-col gap-3">
                    label(attrs: topcoat::view::attributes! { for="common_name" }, "Nom commun aux versions (celui de la liste)")
                    input(attrs: topcoat::view::attributes! { id="common_name" name="name" required=(true) value=(common_name.to_owned()) })
                    label(attrs: topcoat::view::attributes! { for="options" }, "Ce qui distingue les versions — une option par ligne, ses valeurs dans l'ordre proposé aux clients")
                    textarea(attrs: topcoat::view::attributes! { id="options" name="options" rows="3" required=(true) placeholder="Couleur : Noir, Argent\nCapacité : 64 Go, 128 Go" }, (options_text.clone()))
                    if declined {
                        <p class="text-sm text-muted-foreground">"Une valeur occupée par une version ne se retire pas."</p>
                    }
                    <div>button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, if declined { "Enregistrer" } else { "Décliner ce produit" })</div>
                </form>

                if declined {
                    <h3 class="mt-6 text-sm font-medium">(lines.len().to_string()) " version(s)"</h3>
                    if !told_apart {
                        <p class="mt-1 text-sm text-muted-foreground">"Dites d'abord ce qui distingue les versions."</p>
                    } else {
                        if unplaced {
                            <p class="mt-1 text-sm text-destructive">"À placer : choisissez où ce produit se situe."</p>
                            <form method="post" action=(href!(place, ProductId(product_id.to_owned()))) class="mt-2 flex flex-wrap items-end gap-2">
                                <input type="hidden" name="product_id" value=(product_id.to_owned())>
                                for choice in &own {
                                    <div class="flex flex-col gap-1.5">
                                        label(attrs: topcoat::view::attributes! { for=(format!("own_{}", choice.field)) }, (choice.option.clone()))
                                        select(
                                            attrs: topcoat::view::attributes! { id=(format!("own_{}", choice.field)) name=(choice.field.clone()) required=(true) },
                                            <option value="">"— choisir —"</option>
                                            for value in &choice.values { <option value=(value.clone())>(value.clone())</option> }
                                        )
                                    </div>
                                }
                                button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Placer")
                            </form>
                        }
                        if !lines.is_empty() {
                            table(
                                table_header(table_row(
                                    table_head("Produit")
                                    table_head("Place")
                                    table_head("")
                                ))
                                table_body(
                                    for line in &lines {
                                        table_row(
                                            table_cell(
                                                if line.here {
                                                    <span>(line.name.clone())</span>
                                                    <span class="ml-2 text-xs text-muted-foreground">"ce produit"</span>
                                                } else {
                                                    link(href: line.link.clone(), (line.name.clone()))
                                                }
                                                <span class="ml-2 font-mono text-xs text-muted-foreground">(line.sku.clone())</span>
                                                if line.archived { <span class="ml-2 text-xs text-muted-foreground">"archivé"</span> }
                                                if !line.complete { <span class="ml-2 text-xs text-destructive">"à compléter"</span> }
                                            )
                                            table_cell(
                                                <form method="post" action=(href!(place, ProductId(product_id.to_owned()))) class="flex flex-wrap items-center gap-2">
                                                    <input type="hidden" name="product_id" value=(line.product_id.clone())>
                                                    for choice in &line.choices {
                                                        select(
                                                            attrs: topcoat::view::attributes! { name=(choice.field.clone()) required=(true) aria-label=(choice.option.clone()) },
                                                            <option value="" selected=(choice.current.is_none())>(format!("— {} —", choice.option))</option>
                                                            for value in &choice.values {
                                                                <option value=(value.clone()) selected=(choice.current.as_deref() == Some(value.as_str()))>(value.clone())</option>
                                                            }
                                                        )
                                                    }
                                                    button(variant: ButtonVariant::Ghost, attrs: topcoat::view::attributes! { type="submit" }, "Déplacer")
                                                </form>
                                            )
                                            table_cell(
                                                <form method="post" action=(href!(remove, ProductId(product_id.to_owned())))>
                                                    <input type="hidden" name="product_id" value=(line.product_id.clone())>
                                                    button(variant: ButtonVariant::Ghost, attrs: topcoat::view::attributes! { type="submit" }, "Retirer")
                                                </form>
                                            )
                                        )
                                    }
                                )
                            )
                        }

                        <h3 class="mt-6 text-sm font-medium">"Ajouter une version"</h3>
                        <p class="mb-2 text-xs text-muted-foreground">"Un nouveau produit, né de celui-ci : même marque, même catégorie, même descriptif, même prix — à ajuster depuis sa page, avec son stock."</p>
                        <form method="post" action=(href!(add, ProductId(product_id.to_owned()))) class="grid gap-3 sm:grid-cols-2">
                            <div class="flex flex-col gap-1.5">
                                label(attrs: topcoat::view::attributes! { for="version_sku" }, "Référence (SKU)")
                                input(attrs: topcoat::view::attributes! { id="version_sku" name="sku" required=(true) })
                            </div>
                            <div class="flex flex-col gap-1.5">
                                label(attrs: topcoat::view::attributes! { for="version_name" }, "Nom")
                                input(attrs: topcoat::view::attributes! { id="version_name" name="name" required=(true) placeholder=(format!("{} — Argent", product.name)) })
                            </div>
                            for choice in &own {
                                <div class="flex flex-col gap-1.5">
                                    label(attrs: topcoat::view::attributes! { for=(format!("new_{}", choice.field)) }, (choice.option.clone()))
                                    select(
                                        attrs: topcoat::view::attributes! { id=(format!("new_{}", choice.field)) name=(choice.field.clone()) required=(true) class="w-full" },
                                        <option value="">"— choisir —"</option>
                                        for value in &choice.values { <option value=(value.clone())>(value.clone())</option> }
                                    )
                                </div>
                            }
                            <div class="sm:col-span-2">button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Ajouter la version")</div>
                        </form>
                    }
                }
            )
        )
    })
}

#[derive(Debug, Deserialize)]
pub struct DefineForm {
    name: String,
    options: String,
}

/// Declines the product — or, declined already, renames the article and
/// says again what tells its versions apart. The family is opened under the
/// common name's word, the product's reference telling it from a namesake.
#[page(POST "./define")]
pub async fn define(cx: &Cx, Form(form): Form<DefineForm>) -> Result<impl View> {
    let (id, product) = load(cx).await?;
    let catalog = timada_catalog::Command(&app_context::<AdminServices>(cx).executor);
    let defined: std::result::Result<(), CatalogError> = async {
        // Declined already — unless what it claims is over, which frees it.
        let open = match &product.family_id {
            Some(family_id) => match catalog.load_family(family_id).await? {
                Some(family) if !family.dissolved => Some(family.id),
                _ => {
                    catalog.remove_variant(family_id, &id).await?;
                    None
                }
            },
            None => None,
        };
        let family_id = match open {
            Some(family_id) => family_id,
            None => {
                let name = form.name.trim().to_owned();
                let word = timada_core::slug::slugify(&name);
                let opened = catalog
                    .create_family(CreateFamily {
                        name: name.clone(),
                        slug: Some(word.clone()),
                    })
                    .await;
                let family_id = match opened {
                    Err(CatalogError::FamilySlugAlreadyExists(_)) => {
                        catalog
                            .create_family(CreateFamily {
                                name,
                                slug: Some(format!(
                                    "{word}-{}",
                                    timada_core::slug::slugify(&product.sku)
                                )),
                            })
                            .await?
                    }
                    other => other?,
                };
                catalog.join_family(&family_id, &id).await?;
                family_id
            }
        };
        catalog.rename_family(&family_id, &form.name).await?;
        catalog
            .define_family_options(&family_id, parse_options(&form.options))
            .await?;
        Ok(())
    }
    .await;
    Err::<(), _>(see_other(back(cx, &id, defined)?).into())
}

/// The family the product is declined in, or a form from a stale page.
async fn family_of<E: evento::Executor>(
    catalog: &timada_catalog::Command<'_, E>,
    product: &ProductPageView,
) -> std::result::Result<FamilyState, CatalogError> {
    let Some(family_id) = &product.family_id else {
        return Err(CatalogError::FamilyNotFound);
    };
    catalog
        .load_family(family_id)
        .await?
        .ok_or(CatalogError::FamilyNotFound)
}

/// Adds a version: a product born from this one — brand, category, texts,
/// technical sheet and listed price copied —, then placed. Stock is its own.
#[page(POST "./add")]
pub async fn add(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<impl View> {
    let (id, product) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let catalog = timada_catalog::Command(&services.executor);
    let added: std::result::Result<(), CatalogError> = async {
        let family = family_of(&catalog, &product).await?;
        // The place first, before a product is born for nothing.
        let spot = family.place(&values_from(&family, &form))?;
        if family.taken_by(&spot).is_some() {
            return Err(CatalogError::VariantPlaceTaken);
        }
        let sku = form.get("sku").cloned().unwrap_or_default();
        let name = form.get("name").cloned().unwrap_or_default();
        let version = catalog
            .create_product(CreateProduct {
                sku,
                name,
                brand: product.brand.clone(),
                category_path: product.category_path.clone(),
                short_description: product.short_description.clone(),
                warranty_months: product.warranty_months,
            })
            .await?;
        if let Some(category_id) = &product.category_id {
            match catalog.categorise_product(&version, category_id).await {
                Ok(_) | Err(CatalogError::CategoryArchived | CatalogError::CategoryNotFound) => {}
                Err(err) => return Err(err),
            }
        }
        catalog
            .describe_product(
                &version,
                DescribeProduct {
                    long_description: product.long_description.clone(),
                    key_features: product.key_features.clone(),
                },
            )
            .await?;
        if !product.specs.is_empty() {
            catalog
                .specify_product(&version, product.specs.clone())
                .await?;
        }
        // The price it starts at: this product's, in the listed currency.
        // A product without a price gives none; its page asks for one.
        if let Some(price) = load_product_price(&services.executor, price_id(&id)).await? {
            timada_pricing::Command(&services.executor)
                .list_price(ListPrice {
                    product_id: version.clone(),
                    price_incl_tax: price.price_incl_tax.clone(),
                    vat_rate_bp: price.vat_rate_bp,
                    eco_participation: price.eco_participation.clone(),
                })
                .await
                .map_err(anyhow::Error::from)?;
        }
        catalog.place_variant(&family.id, &version, spot).await?;
        Ok(())
    }
    .await;
    Err::<(), _>(see_other(back(cx, &id, added)?).into())
}

/// Places — or moves — a version: `product_id` and `value_<n>` per option.
#[page(POST "./place")]
pub async fn place(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<impl View> {
    let (id, product) = load(cx).await?;
    let catalog = timada_catalog::Command(&app_context::<AdminServices>(cx).executor);
    let placed: std::result::Result<(), CatalogError> = async {
        let family = family_of(&catalog, &product).await?;
        let version = form.get("product_id").cloned().unwrap_or_default();
        catalog
            .place_variant(&family.id, &version, values_from(&family, &form))
            .await?;
        Ok(())
    }
    .await;
    Err::<(), _>(see_other(back(cx, &id, placed)?).into())
}

#[derive(Debug, Deserialize)]
pub struct RemoveForm {
    product_id: String,
}

/// Takes a version out — this product included; it stays a product, on its
/// own. The last one out ends the declination.
#[page(POST "./remove")]
pub async fn remove(cx: &Cx, Form(form): Form<RemoveForm>) -> Result<impl View> {
    let (id, product) = load(cx).await?;
    let catalog = timada_catalog::Command(&app_context::<AdminServices>(cx).executor);
    let removed: std::result::Result<(), CatalogError> = async {
        let family = family_of(&catalog, &product).await?;
        catalog.remove_variant(&family.id, &form.product_id).await?;
        let emptied = catalog
            .load_family(&family.id)
            .await?
            .is_some_and(|family| family.variants.is_empty() && !family.dissolved);
        if emptied {
            // This product may still claim it without a place: freed too.
            if form.product_id != id {
                catalog.remove_variant(&family.id, &id).await?;
            }
            catalog.dissolve_family(&family.id).await?;
        }
        Ok(())
    }
    .await;
    Err::<(), _>(see_other(back(cx, &id, removed)?).into())
}
