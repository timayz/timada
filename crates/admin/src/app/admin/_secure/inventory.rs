//! `/{mount}/inventory`: stock levels per product and location, receiving
//! units, and starting to track a product somewhere.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::{load_product_page, product_id, products_by_ids};
use timada_inventory::{
    InventoryError, ListStock, RegisterStockItem, StockLocation, count_stock, list_stock,
    load_stock_availability,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page, query_params, query_params as query},
    view::{View, view},
};

use super::products::{field, product_id as product_page};
use crate::{
    components::{
        button::{ButtonVariant, button, button_variants},
        card::{card, card_content},
        input::input,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{empty_state, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct InventoryQuery {
    page: Option<u32>,
    /// Keeps the items with fewer available units than this.
    below: Option<u32>,
}

/// One line of the listing, ready to render.
struct StockLine {
    stock_item_id: String,
    product_link: String,
    product_name: String,
    sku: String,
    location: String,
    on_hand: i64,
    reserved: i64,
    available: i64,
}

fn location_label(key: &str) -> String {
    match key.strip_prefix("store:") {
        Some(store_id) => format!("Boutique {store_id}"),
        None => "Entrepôt".to_owned(),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<InventoryQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let services = app_context::<AdminServices>(cx);
    let rows = list_stock(
        &services.db,
        &ListStock {
            available_below: query.below,
            limit: PAGE_SIZE,
            offset: (page - 1) * PAGE_SIZE,
        },
    )
    .await?;
    let total = count_stock(&services.db, query.below).await?;

    let product_ids: Vec<String> = rows.iter().map(|r| r.product_id.clone()).collect();
    let products: HashMap<String, (String, String)> = products_by_ids(&services.db, &product_ids)
        .await?
        .into_iter()
        .map(|p| (p.id, (p.name, p.sku)))
        .collect();
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        // The list table lags behind the event store: the levels shown are
        // the item's own, so a receipt is visible as soon as it is recorded.
        let levels = load_stock_availability(&services.executor, &row.stock_item_id)
            .await?
            .map(|s| {
                (
                    i64::from(s.on_hand),
                    i64::from(s.reserved),
                    i64::from(s.available),
                )
            })
            .unwrap_or((row.on_hand, row.reserved, row.available));
        let (product_name, sku) = products
            .get(&row.product_id)
            .cloned()
            .unwrap_or_else(|| (row.product_id.clone(), String::new()));
        lines.push(StockLine {
            product_link: href!(
                product_page::show,
                product_page::ProductId(row.product_id.clone())
            )
            .resolve(cx),
            stock_item_id: row.stock_item_id,
            product_name,
            sku,
            location: location_label(&row.location),
            on_hand: levels.0,
            reserved: levels.1,
            available: levels.2,
        });
    }

    Ok(view! {
        page_header(
            title: "Stock",
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="below" class="text-muted-foreground">"Disponible inférieur à"</label>
                input(attrs: topcoat::view::attributes! { id="below" type="number" name="below" min="1" class="w-24" value=(query.below.map(|b| b.to_string()).unwrap_or_default()) })
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Filtrer"</button>
            </form>
            <a href=(href!(new)) class=(button_variants(ButtonVariant::Primary, Default::default()))>"Suivre un produit"</a>
        )
        if lines.is_empty() {
            empty_state(message: "Aucun article en stock suivi.")
        } else {
            table(
                table_header(table_row(
                    table_head("Produit") table_head("Emplacement")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "En stock")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Réservé")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Disponible")
                    table_head("Réception")
                ))
                table_body(
                    for line in &lines { stock_row(line: line) }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[topcoat::view::component]
async fn stock_row(cx: &Cx, line: &StockLine) -> Result<impl View> {
    let quantity_id = format!("quantity-{}", line.stock_item_id);
    let quantity_label = format!("Unités reçues pour {}", line.product_name);
    Ok(view! {
        table_row(
            table_cell(
                <a href=(line.product_link.clone()) class="underline-offset-4 hover:underline">(line.product_name.clone())</a>
                <span class="ml-2 font-mono text-xs text-muted-foreground">(line.sku.clone())</span>
            )
            table_cell((line.location.clone()))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (line.on_hand.to_string()))
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (line.reserved.to_string()))
            table_cell(attrs: topcoat::view::attributes! { class="text-right font-semibold tabular-nums" }, (line.available.to_string()))
            table_cell(
                <form method="post" action=(href!(receive_units).resolve(cx)) class="flex items-center gap-2">
                    <input type="hidden" name="stock_item_id" value=(line.stock_item_id.clone())>
                    input(attrs: topcoat::view::attributes! { id=(quantity_id) type="number" name="quantity" min="1" required=(true) class="w-20" aria-label=(quantity_label) })
                    button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Recevoir")
                </form>
            )
        )
    })
}

#[derive(Debug, Deserialize)]
pub struct ReceiveForm {
    stock_item_id: String,
    quantity: u32,
}

#[page(POST "./receive")]
pub async fn receive_units(cx: &Cx, Form(form): Form<ReceiveForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    timada_inventory::Command(&services.executor)
        .receive_stock(&form.stock_item_id, form.quantity)
        .await?;
    Err::<(), _>(see_other(href!(index).resolve(cx)).into())
}

#[derive(Debug, Deserialize)]
pub struct NewStockItemForm {
    sku: String,
    store_id: String,
    quantity: String,
}

#[page("./new")]
pub async fn new() -> Result<impl View> {
    Ok(view! { new_stock_item_form(error: None) })
}

#[page(POST "./new")]
pub async fn create(cx: &Cx, Form(form): Form<NewStockItemForm>) -> Result<impl View> {
    let error = match track(cx, form).await? {
        Ok(()) => return Err(see_other(href!(index).resolve(cx)).into()),
        Err(error) => error,
    };
    Ok(view! { new_stock_item_form(error: Some(error)) })
}

/// Starts tracking the product at the location and books the first receipt.
async fn track(cx: &Cx, form: NewStockItemForm) -> Result<std::result::Result<(), String>> {
    let services = app_context::<AdminServices>(cx);
    let product_id = product_id(&form.sku.trim().to_uppercase());
    if load_product_page(&services.executor, &product_id)
        .await?
        .is_none()
    {
        return Ok(Err("Aucun produit avec cette référence.".into()));
    }
    let quantity: u32 = match form.quantity.trim() {
        "" => 0,
        raw => match raw.parse() {
            Ok(quantity) => quantity,
            Err(_) => return Ok(Err("Quantité : nombre entier attendu.".into())),
        },
    };
    let store_id = form.store_id.trim();
    let location = if store_id.is_empty() {
        StockLocation::Warehouse
    } else {
        StockLocation::Store {
            store_id: store_id.to_owned(),
        }
    };

    let inventory = timada_inventory::Command(&services.executor);
    let stock_item_id = match inventory
        .register_stock_item(RegisterStockItem {
            product_id,
            location,
        })
        .await
    {
        Ok(id) => id,
        Err(InventoryError::AlreadyRegistered) => {
            return Ok(Err(
                "Ce produit est déjà suivi à cet emplacement : utilisez « Recevoir ».".into(),
            ));
        }
        Err(err) => return Ok(Err(err.to_string())),
    };
    if quantity > 0 {
        inventory.receive_stock(&stock_item_id, quantity).await?;
    }
    Ok(Ok(()))
}

#[topcoat::view::component]
async fn new_stock_item_form(cx: &Cx, error: Option<String>) -> Result<impl View> {
    Ok(view! {
        page_header(title: "Suivre un produit")
        <div class="max-w-2xl">
            card(card_content(
                <form method="post" action=(href!(create).resolve(cx)) class="grid gap-4 sm:grid-cols-2">
                    field(name: "sku", label_text: "Référence (SKU)", attrs: topcoat::view::attributes! { required=(true) autocomplete="off" })
                    field(name: "store_id", label_text: "Boutique (vide = entrepôt)", attrs: topcoat::view::attributes! {})
                    field(name: "quantity", label_text: "Quantité reçue", attrs: topcoat::view::attributes! { type="number" min="0" value="0" })
                    if let Some(error) = &error {
                        <p role="alert" class="text-sm text-destructive sm:col-span-2">(error.clone())</p>
                    }
                    <div class="sm:col-span-2">
                        button(attrs: topcoat::view::attributes! { type="submit" }, "Suivre")
                    </div>
                </form>
            ))
        </div>
    })
}
