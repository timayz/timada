//! Storefront pages. Explicit paths on purpose: the host must not use
//! `module_router!()` while the admin's module-derived pages are linked in.

use timada_catalog::{ListProducts, ProductListRow, list_products, load_product_page};
use timada_inventory::{StockLocation, stock_item_id};
use timada_pricing::{load_product_price, price_id};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::RouterErrorExt, href, page, path_param, path_param as param},
    view::{Child, View, component, view},
};

use crate::Store;

#[component]
async fn document(title: &str, child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html lang="fr">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title) " · Timada demo"</title>
                <style>
                    "body{font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem;line-height:1.5}"
                    "header{display:flex;justify-content:space-between;align-items:baseline;border-bottom:1px solid #ddd;padding-bottom:.5rem}"
                    "a{color:#0b5fa5}ul{padding-left:1.2rem}.price{font-size:1.5rem;font-weight:600}.muted{color:#666}"
                </style>
            </head>
            <body>
                <header>
                    <a href=(href!(home))><strong>"Timada demo"</strong></a>
                    <a href="/admin" class="muted">"Administration"</a>
                </header>
                <main>(child)</main>
            </body>
        </html>
    })
}

#[page("/")]
async fn home(cx: &Cx) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    let products = list_products(&store.db, &ListProducts::default()).await?;
    Ok(view! {
        document(
            title: "Catalogue",
            <h1>"Catalogue"</h1>
            if products.is_empty() {
                <p class="muted">"Aucun produit. Lancez " <code>"cargo run -p demo -- --seed"</code> "."</p>
            } else {
                <ul>
                    for product in &products { product_item(product: product) }
                </ul>
            }
        )
    })
}

#[component]
async fn product_item(cx: &Cx, product: &ProductListRow) -> Result<impl View> {
    let link = href!(product_page, ProductId(product.id.clone())).resolve(cx);
    Ok(view! {
        <li><a href=(link)>(product.name.clone())</a> <span class="muted">(product.sku.clone())</span></li>
    })
}

path_param!(product_id: String, error = not_found);

#[page("/p/{product_id}")]
async fn product_page(cx: &Cx) -> Result<impl View> {
    let id = param::<ProductId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let product = load_product_page(&store.executor, &id)
        .await?
        .ok_or_not_found()?;
    let price = load_product_price(&store.executor, price_id(&id)).await?;
    let stock = timada_inventory::load_stock_availability(
        &store.executor,
        stock_item_id(&id, &StockLocation::Warehouse),
    )
    .await?;
    let availability = match stock {
        Some(s) if s.available > 0 => format!("En stock ({} disponibles)", s.available),
        _ => "Rupture".to_owned(),
    };

    Ok(view! {
        document(
            title: &product.name,
            <p class="muted">(product.category_path.join(" > "))</p>
            <h1>(product.name.clone())</h1>
            <p>(product.short_description.clone())</p>
            match &price {
                Some(price) => {
                    <p class="price">(format!("{},{:02} €", price.price_incl_tax.minor / 100, price.price_incl_tax.minor % 100))</p>
                    if let Some(amount) = &price.installment_amount {
                        <p class="muted">"ou 3 × " (format!("{},{:02} €", amount.minor / 100, amount.minor % 100))</p>
                    }
                }
                None => <p class="muted">"Prix indisponible"</p>,
            }
            <p>(availability) " · garantie " (product.warranty_months.to_string()) " mois"</p>
            if !product.key_features.is_empty() {
                <h2>"Caractéristiques principales"</h2>
                <ul>for feature in &product.key_features { <li>(feature.clone())</li> }</ul>
            }
            if !product.long_description.is_empty() { <p>(product.long_description.clone())</p> }
        )
    })
}
