//! `/{mount}/products`: the catalog, plus creating a product with its price.

pub mod product_id;

use serde::Deserialize;
use timada_catalog::{
    Brand, CreateProduct, ListProducts, ProductListRow, category_lineage, count_products,
    list_products,
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
    app::admin::_secure::categories::{category_options, category_select},
    components::{
        button::{ButtonVariant, button, button_variants},
        card::{card, card_content},
        input::input,
        label::label,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::{AdminConfig, AdminServices},
    ui::{
        empty_state, filter_bar, form_error, link, page_header, pagination, table_card, text_field,
    },
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
            filter_bar(
                submit: "Rechercher",
                input(attrs: topcoat::view::attributes! { type="search" name="q" placeholder="Nom ou référence" value=(query.q.clone().unwrap_or_default()) })
                <label class="flex h-9 items-center gap-1.5 text-muted-foreground">
                    <input type="checkbox" name="archived" value="1" checked=(include_archived)> "Archivés"
                </label>
            )
            <a href=(href!(new)) class=(button_variants(ButtonVariant::Primary, Default::default()))>"Nouveau produit"</a>
        )
        if rows.is_empty() {
            empty_state(message: "Aucun produit.")
        } else {
            table_card(
                table(
                    table_header(table_row(
                        table_head("Référence") table_head("Nom") table_head("Marque") table_head("Catégorie") table_head("État")
                    ))
                    table_body(
                        for row in &rows { product_row(row: row) }
                    )
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[topcoat::view::component]
async fn product_row(cx: &Cx, row: &ProductListRow) -> Result<impl View> {
    let target = href!(product_id::show, product_id::ProductId(row.id.clone())).resolve(cx);
    Ok(view! {
        table_row(
            table_cell(link(href: target, class: "font-mono text-xs", (row.sku.clone())))
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
    category_id: String,
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
    let slug = timada_core::slug::slugify(&form.brand);
    // The label a product is created with is its category's breadcrumb.
    let category_id = Some(form.category_id).filter(|id| !id.is_empty());
    let category_path = match &category_id {
        Some(id) => category_lineage(&services.db, id)
            .await?
            .into_iter()
            .map(|category| category.name)
            .collect(),
        None => Vec::new(),
    };
    // A product is listed in the shop's base currency; the others are given
    // from its page.
    let base_currency = app_context::<AdminConfig>(cx).currencies.base();
    let catalog = timada_catalog::Command(&services.executor);
    let created = catalog
        .create_product(CreateProduct {
            sku: form.sku,
            name: form.name,
            brand: Brand {
                name: form.brand.trim().to_owned(),
                slug,
            },
            category_path,
            short_description: form.short_description,
            warranty_months: form.warranty_months,
        })
        .await;
    let error = match created {
        Ok(id) => {
            if let Some(category_id) = category_id
                && let Err(err) = catalog.categorise_product(&id, category_id).await
            {
                // Archived in between: the product exists, to be filed later.
                tracing::warn!(product_id = %id, %err, "new product not filed");
            }
            let priced = timada_pricing::Command(&services.executor)
                .list_price(ListPrice {
                    product_id: id.clone(),
                    price_incl_tax: Money::new(form.price_cents, base_currency),
                    vat_rate_bp: form.vat_rate_bp,
                    eco_participation: Money::new(form.eco_participation_cents, base_currency),
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
    let categories = category_options(cx, None).await?;
    Ok(view! {
        page_header(title: "Nouveau produit")
        <div class="max-w-2xl">
            card(card_content(
                <form method="post" action=(href!(create).resolve(cx)) class="grid gap-4 sm:grid-cols-2">
                    text_field(name: "sku", label_text: "Référence (SKU)", attrs: topcoat::view::attributes! { required=(true) })
                    text_field(name: "name", label_text: "Nom", attrs: topcoat::view::attributes! { required=(true) })
                    text_field(name: "brand", label_text: "Marque", attrs: topcoat::view::attributes! { required=(true) })
                    <div class="flex flex-col gap-1.5">
                        label(attrs: topcoat::view::attributes! { for="category_id" }, "Catégorie")
                        category_select(name: "category_id", options: &categories, selected: None, none_label: Some("— à ranger plus tard —"))
                    </div>
                    <div class="sm:col-span-2">
                        text_field(name: "short_description", label_text: "Description courte", attrs: topcoat::view::attributes! {})
                    </div>
                    text_field(name: "warranty_months", label_text: "Garantie (mois)", attrs: topcoat::view::attributes! { type="number" min="0" value="24" })
                    text_field(name: "price_cents", label_text: "Prix TTC (centimes)", attrs: topcoat::view::attributes! { type="number" min="1" required=(true) })
                    text_field(name: "vat_rate_bp", label_text: "TVA (points de base, 2000 = 20 %)", attrs: topcoat::view::attributes! { type="number" min="0" value="2000" })
                    text_field(name: "eco_participation_cents", label_text: "Éco-participation (centimes)", attrs: topcoat::view::attributes! { type="number" min="0" value="0" })
                    if let Some(error) = &error {
                        form_error(class: "sm:col-span-2", (error.clone()))
                    }
                    <div class="sm:col-span-2">
                        button(attrs: topcoat::view::attributes! { type="submit" }, "Créer")
                    </div>
                </form>
            ))
        </div>
    })
}
