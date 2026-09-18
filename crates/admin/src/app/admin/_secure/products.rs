//! `/{mount}/products`: the catalog, plus creating a product with its price.

pub mod product_id;

use serde::Deserialize;
use timada_catalog::{
    Brand, CreateProduct, ListProducts, ProductListRow, count_products, list_products,
};
use timada_core::Money;
use timada_pricing::ListPrice;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page, query_params, query_params as query},
    view::{View, view},
};

use crate::{
    components::{
        button::{ButtonVariant, button, button_variants},
        card::{card, card_content},
        input::input,
        label::label,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{empty_state, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct ProductsQuery {
    page: Option<u32>,
    q: Option<String>,
    archived: Option<String>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<ProductsQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let include_archived = query.archived.as_deref() == Some("1");
    let db = &app_context::<AdminServices>(cx).db;
    let rows = list_products(
        db,
        &ListProducts {
            q: query.q.clone(),
            include_archived,
            limit: PAGE_SIZE,
            offset: (page - 1) * PAGE_SIZE,
        },
    )
    .await?;
    let total = count_products(db, query.q.as_deref(), include_archived).await?;

    Ok(view! {
        page_header(
            title: "Produits",
            <form method="get" class="flex items-center gap-2 text-sm">
                input(attrs: topcoat::view::attributes! { type="search" name="q" placeholder="Nom ou référence" value=(query.q.clone().unwrap_or_default()) })
                <label class="flex items-center gap-1 text-muted-foreground">
                    <input type="checkbox" name="archived" value="1" checked=(include_archived)> "Archivés"
                </label>
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Rechercher"</button>
            </form>
            <a href=(href!(new)) class=(button_variants(ButtonVariant::Primary, Default::default()))>"Nouveau produit"</a>
        )
        if rows.is_empty() {
            empty_state(message: "Aucun produit.")
        } else {
            table(
                table_header(table_row(
                    table_head("Référence") table_head("Nom") table_head("Marque") table_head("Catégorie") table_head("État")
                ))
                table_body(
                    for row in &rows { product_row(row: row) }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[topcoat::view::component]
async fn product_row(cx: &Cx, row: &ProductListRow) -> Result<impl View> {
    let link = href!(product_id::show, product_id::ProductId(row.id.clone())).resolve(cx);
    Ok(view! {
        table_row(
            table_cell(<a href=(link) class="font-mono text-xs underline-offset-4 hover:underline">(row.sku.clone())</a>)
            table_cell((row.name.clone()))
            table_cell((row.brand_slug.clone()))
            table_cell(<span class="text-muted-foreground">(row.category_path.clone())</span>)
            table_cell(if row.archived { "Archivé" } else { "Actif" })
        )
    })
}

#[derive(Debug, Deserialize)]
pub struct NewProductForm {
    sku: String,
    name: String,
    brand: String,
    category_path: String,
    short_description: String,
    warranty_months: u16,
    price_cents: i64,
    vat_rate_bp: u16,
    eco_participation_cents: i64,
}

#[page("./new")]
pub async fn new() -> Result<impl View> {
    Ok(view! { new_product_form(error: None) })
}

#[page(POST "./new")]
pub async fn create(cx: &Cx, Form(form): Form<NewProductForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let slug = form.brand.trim().to_lowercase().replace(' ', "-");
    let created = timada_catalog::Command(&services.executor)
        .create_product(CreateProduct {
            sku: form.sku,
            name: form.name,
            brand: Brand {
                name: form.brand.trim().to_owned(),
                slug,
            },
            category_path: form
                .category_path
                .split('>')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            short_description: form.short_description,
            warranty_months: form.warranty_months,
        })
        .await;
    let error = match created {
        Ok(id) => {
            let priced = timada_pricing::Command(&services.executor)
                .list_price(ListPrice {
                    product_id: id.clone(),
                    price_incl_tax: Money::eur(form.price_cents),
                    vat_rate_bp: form.vat_rate_bp,
                    eco_participation: Money::eur(form.eco_participation_cents),
                })
                .await;
            match priced {
                Ok(_) => {
                    let show = href!(product_id::show, product_id::ProductId(id)).resolve(cx);
                    return Err(see_other(show).into());
                }
                Err(err) => err.to_string(),
            }
        }
        Err(err) => err.to_string(),
    };
    Ok(view! { new_product_form(error: Some(error)) })
}

#[topcoat::view::component]
async fn new_product_form(cx: &Cx, error: Option<String>) -> Result<impl View> {
    Ok(view! {
        page_header(title: "Nouveau produit")
        <div class="max-w-2xl">
            card(card_content(
                <form method="post" action=(href!(create).resolve(cx)) class="grid gap-4 sm:grid-cols-2">
                    field(name: "sku", label_text: "Référence (SKU)", attrs: topcoat::view::attributes! { required=(true) })
                    field(name: "name", label_text: "Nom", attrs: topcoat::view::attributes! { required=(true) })
                    field(name: "brand", label_text: "Marque", attrs: topcoat::view::attributes! { required=(true) })
                    field(name: "category_path", label_text: "Catégorie (A > B > C)", attrs: topcoat::view::attributes! {})
                    <div class="sm:col-span-2">
                        field(name: "short_description", label_text: "Description courte", attrs: topcoat::view::attributes! {})
                    </div>
                    field(name: "warranty_months", label_text: "Garantie (mois)", attrs: topcoat::view::attributes! { type="number" min="0" value="24" })
                    field(name: "price_cents", label_text: "Prix TTC (centimes)", attrs: topcoat::view::attributes! { type="number" min="1" required=(true) })
                    field(name: "vat_rate_bp", label_text: "TVA (points de base, 2000 = 20 %)", attrs: topcoat::view::attributes! { type="number" min="0" value="2000" })
                    field(name: "eco_participation_cents", label_text: "Éco-participation (centimes)", attrs: topcoat::view::attributes! { type="number" min="0" value="0" })
                    if let Some(error) = &error {
                        <p role="alert" class="text-sm text-destructive sm:col-span-2">(error.clone())</p>
                    }
                    <div class="sm:col-span-2">
                        button(attrs: topcoat::view::attributes! { type="submit" }, "Créer")
                    </div>
                </form>
            ))
        </div>
    })
}

/// A labelled input; `attrs` carries type/required/value overrides.
#[topcoat::view::component]
pub async fn field(
    cx: &Cx,
    name: &str,
    label_text: &str,
    mut attrs: topcoat::view::Attributes,
) -> Result<impl View> {
    attrs.insert(cx, "id", name.to_owned());
    attrs.insert(cx, "name", name.to_owned());
    Ok(view! {
        <div class="flex flex-col gap-1.5">
            label(attrs: topcoat::view::attributes! { for=(name) }, (label_text))
            input(attrs: attrs)
        </div>
    })
}
