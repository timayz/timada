//! `/{mount}/products/{product_id}`: content, price, stock and the editing actions.

use serde::Deserialize;
use timada_catalog::{
    CatalogError, DescribeProduct, ProductPageView, category_lineage, load_product_page,
};
use timada_core::Money;
use timada_inventory::{StockLocation, stock_item_id};
use timada_pricing::{InstallmentOffer, load_product_price, price_id};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param,
    },
    view::{View, view},
};

use crate::{
    app::admin::_secure::categories::{category_options, category_select},
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
        textarea::textarea,
    },
    config::AdminServices,
    ui::{money, page_header},
};

path_param!(pub product_id: String, error = not_found);

async fn load(cx: &Cx) -> Result<(String, ProductPageView)> {
    let id = param::<ProductId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let product = load_product_page(&services.executor, &id)
        .await?
        .ok_or_not_found()?;
    Ok((id, product))
}

fn back(cx: &Cx, id: &str) -> String {
    href!(show, ProductId(id.to_owned())).resolve(cx)
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let (id, product) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let price = load_product_price(&services.executor, price_id(&id)).await?;
    let stock = timada_inventory::load_stock_availability(
        &services.executor,
        stock_item_id(&id, &StockLocation::Warehouse),
    )
    .await?;
    let rating = timada_review::product_rating(&services.db, &id).await?;
    // Where the product is filed; the label it was created with until then.
    let filed_under = match &product.category_id {
        Some(category_id) => category_lineage(&services.db, category_id)
            .await?
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(" > "),
        None => product.category_path.join(" > "),
    };
    let categories = category_options(cx, None).await?;

    Ok(view! {
        page_header(
            title: &product.name,
            if product.archived { <span class="text-sm text-muted-foreground">"Archivé"</span> }
        )
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">
            (product.brand.name.clone()) " · " (product.sku.clone()) " · " (filed_under)
            " · garantie " (product.warranty_months.to_string()) " mois"
        </p>

        <div class="grid gap-6 lg:grid-cols-3">
            <div class="flex flex-col gap-6 lg:col-span-2">
                card(
                    card_header(card_title("Descriptif"))
                    card_content(
                        <form method="post" action=(href!(describe, ProductId(id.clone()))) class="flex flex-col gap-4">
                            <div class="flex flex-col gap-1.5">
                                label(attrs: topcoat::view::attributes! { for="short_description" }, "Description courte")
                                input(attrs: topcoat::view::attributes! { id="short_description" value=(product.short_description.clone()) disabled=(true) })
                            </div>
                            <div class="flex flex-col gap-1.5">
                                label(attrs: topcoat::view::attributes! { for="long_description" }, "Description")
                                textarea(attrs: topcoat::view::attributes! { id="long_description" name="long_description" rows="6" }, (product.long_description.clone()))
                            </div>
                            <div class="flex flex-col gap-1.5">
                                label(attrs: topcoat::view::attributes! { for="key_features" }, "Caractéristiques principales (une par ligne)")
                                textarea(attrs: topcoat::view::attributes! { id="key_features" name="key_features" rows="6" }, (product.key_features.join("\n")))
                            </div>
                            if !product.archived {
                                <div>button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer")</div>
                            }
                        </form>
                    )
                )
            </div>
            <div class="flex flex-col gap-6">
                card(
                    card_header(card_title("Prix"))
                    card_content(
                        match &price {
                            Some(price) => {
                                <dl class="mb-4 flex flex-col gap-1 text-sm">
                                    <div class="flex justify-between"><dt class="text-muted-foreground">"TTC"</dt><dd class="tabular-nums">(money(&price.price_incl_tax))</dd></div>
                                    <div class="flex justify-between"><dt class="text-muted-foreground">"HT"</dt><dd class="tabular-nums">(money(&price.price_excl_tax))</dd></div>
                                    <div class="flex justify-between"><dt class="text-muted-foreground">"Éco-part."</dt><dd class="tabular-nums">(money(&price.eco_participation))</dd></div>
                                    if let Some(amount) = &price.installment_amount {
                                        <div class="flex justify-between"><dt class="text-muted-foreground">"Paiement en plusieurs fois"</dt><dd class="tabular-nums">(money(amount))</dd></div>
                                    }
                                </dl>
                                if !price.withdrawn && !product.archived {
                                    <form method="post" action=(href!(change_price, ProductId(id.clone()))) class="flex gap-2">
                                        input(attrs: topcoat::view::attributes! { name="price_cents" type="number" min="1" required=(true) placeholder="Nouveau prix TTC (centimes)" })
                                        button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Modifier")
                                    </form>
                                    if price.installment.is_none() {
                                        <form method="post" action=(href!(offer_installments, ProductId(id.clone()))) class="mt-2 flex gap-2">
                                            input(attrs: topcoat::view::attributes! { name="fee_cents" type="number" min="0" value="479" required=(true) placeholder="Frais (centimes)" })
                                            button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Proposer 3x")
                                        </form>
                                    }
                                }
                            }
                            None => <p class="text-sm text-muted-foreground">"Aucun prix."</p>,
                        }
                    )
                )
                card(
                    card_header(card_title("Stock & avis"))
                    card_content(
                        <dl class="flex flex-col gap-1 text-sm">
                            <div class="flex justify-between"><dt class="text-muted-foreground">"Disponible (entrepôt)"</dt>
                                <dd>(stock.as_ref().map(|s| s.available.to_string()).unwrap_or_else(|| "—".into()))</dd></div>
                            <div class="flex justify-between"><dt class="text-muted-foreground">"Réservé"</dt>
                                <dd>(stock.as_ref().map(|s| s.reserved.to_string()).unwrap_or_else(|| "—".into()))</dd></div>
                            <div class="flex justify-between"><dt class="text-muted-foreground">"Avis"</dt>
                                <dd>(rating.review_count.to_string())
                                    if let Some(avg) = rating.average_rating { " · " (format!("{avg:.1}")) " / 5" }
                                </dd></div>
                        </dl>
                    )
                )
                if !product.archived {
                    card(
                        card_header(card_title("Catégorie"))
                        card_content(
                            if categories.is_empty() {
                                <p class="text-sm text-muted-foreground">"Aucune catégorie ouverte : créez-en une dans la section Catégories."</p>
                            } else {
                                <form method="post" action=(href!(categorise, ProductId(id.clone()))) class="flex flex-col gap-3">
                                    label(attrs: topcoat::view::attributes! { for="category_id" }, "Rangé sous")
                                    category_select(name: "category_id", options: &categories, selected: product.category_id.as_deref(), none_label: product.category_id.is_none().then_some("— non rangé —"))
                                    <div>button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Ranger")</div>
                                </form>
                            }
                        )
                    )
                    <form method="post" action=(href!(archive, ProductId(id.clone())))>
                        button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Archiver le produit")
                    </form>
                }
            </div>
        </div>
    })
}

#[derive(Debug, Deserialize)]
pub struct DescribeForm {
    long_description: String,
    key_features: String,
}

#[page(POST "./describe")]
pub async fn describe(cx: &Cx, Form(form): Form<DescribeForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_catalog::Command(&services.executor)
        .describe_product(
            &id,
            DescribeProduct {
                long_description: form.long_description,
                key_features: form
                    .key_features
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect(),
            },
        )
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

#[derive(Debug, Deserialize)]
pub struct PriceForm {
    price_cents: i64,
}

#[page(POST "./price")]
pub async fn change_price(cx: &Cx, Form(form): Form<PriceForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_pricing::Command(&services.executor)
        .change_price(price_id(&id), Money::eur(form.price_cents))
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

#[derive(Debug, Deserialize)]
pub struct InstallmentsForm {
    fee_cents: i64,
}

#[page(POST "./installments")]
pub async fn offer_installments(cx: &Cx, Form(form): Form<InstallmentsForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_pricing::Command(&services.executor)
        .attach_installment_offer(
            price_id(&id),
            InstallmentOffer {
                count: 3,
                fee: Money::eur(form.fee_cents),
            },
        )
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

#[page(POST "./archive")]
pub async fn archive(cx: &Cx) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    timada_catalog::Command(&services.executor)
        .archive_product(&id)
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

#[derive(Debug, Deserialize)]
pub struct CategoriseForm {
    category_id: String,
}

/// Files the product under a category; a category archived in the meantime
/// leaves it where it was.
#[page(POST "./categorise")]
pub async fn categorise(cx: &Cx, Form(form): Form<CategoriseForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    if !form.category_id.is_empty() {
        let filed = timada_catalog::Command(&services.executor)
            .categorise_product(&id, form.category_id)
            .await;
        match filed {
            Ok(_) | Err(CatalogError::CategoryArchived | CatalogError::CategoryNotFound) => {}
            Err(err) => return Err(anyhow::Error::from(err).into()),
        }
    }
    Err::<(), _>(see_other(back(cx, &id)).into())
}
