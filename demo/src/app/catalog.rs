//! `/` and `/p/{product_id}`: the catalogue and the product page.

use timada_catalog::{ListProducts, ProductListRow, list_products, load_product_page};
use timada_inventory::{StockLocation, stock_item_id};
use timada_pricing::{load_product_price, price_id};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::RouterErrorExt, href, page, path_param, path_param as param},
    view::{View, component, view},
};

use super::{cart, document, format::money};
use crate::Store;

#[page("/")]
pub async fn home(cx: &Cx) -> Result<impl View> {
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
        <li><a href=(link)>(product.name.clone())</a> " " <span class="muted">(product.sku.clone())</span></li>
    })
}

path_param!(pub product_id: String, error = not_found);

#[page("/p/{product_id}")]
pub async fn product_page(cx: &Cx) -> Result<impl View> {
    let id = param::<ProductId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let product = load_product_page(&store.executor, &id)
        .await?
        .ok_or_not_found()?;
    let price = load_product_price(&store.executor, price_id(&id))
        .await?
        .filter(|p| !p.withdrawn);
    let available = available_stock(store, &id).await?;
    let availability = if available > 0 {
        format!("En stock ({available} disponibles)")
    } else {
        "Rupture".to_owned()
    };

    Ok(view! {
        document(
            title: &product.name,
            <p class="muted">(product.category_path.join(" > "))</p>
            <h1>(product.name.clone())</h1>
            <p>(product.short_description.clone())</p>
            match &price {
                Some(price) => {
                    <p class="price">(money(&price.price_incl_tax))</p>
                    if let Some(amount) = &price.installment_amount {
                        <p class="muted">"ou 3 × " (money(amount))</p>
                    }
                }
                None => <p class="muted">"Prix indisponible"</p>,
            }
            <p>(availability) " · garantie " (product.warranty_months.to_string()) " mois"</p>
            if price.is_some() && available > 0 && !product.archived {
                <form method="post" action=(href!(cart::add))>
                    <input type="hidden" name="product_id" value=(id.clone())>
                    <label for="quantity">"Quantité"</label>
                    " "
                    <input id="quantity" name="quantity" type="number" min="1" max=(available.to_string()) value="1" required=(true)>
                    " "
                    <button type="submit">"Ajouter au panier"</button>
                </form>
            }
            if !product.key_features.is_empty() {
                <h2>"Caractéristiques principales"</h2>
                <ul>for feature in &product.key_features { <li>(feature.clone())</li> }</ul>
            }
            if !product.long_description.is_empty() { <p>(product.long_description.clone())</p> }
        )
    })
}

/// Units the warehouse can still promise for a product.
pub async fn available_stock(store: &Store, product_id: &str) -> anyhow::Result<u32> {
    let stock = timada_inventory::load_stock_availability(
        &store.executor,
        stock_item_id(product_id, &StockLocation::Warehouse),
    )
    .await?;
    Ok(stock.map_or(0, |s| s.available))
}
