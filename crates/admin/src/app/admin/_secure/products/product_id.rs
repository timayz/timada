//! `/{mount}/products/{product_id}`: content, price, stock and the editing actions.

use serde::Deserialize;
use timada_catalog::{
    CatalogError, DescribeProduct, ProductPageView, Spec, category_lineage, load_product_page,
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
    app::admin::_secure::{
        categories::{category_options, category_select},
        families::family_id::{FamilyId, show as show_family},
    },
    auth::Section,
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
        textarea::textarea,
    },
    config::{AdminConfig, AdminServices},
    ui::{detail_grid, detail_main, money, page_header},
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
    // The shop's other currencies with what the product costs in each:
    // `(currency, price or "non vendu", cents for the field)`.
    let other_prices: Vec<(String, String, String)> = app_context::<AdminConfig>(cx)
        .currencies
        .all()
        .filter(|currency| {
            price
                .as_ref()
                .is_some_and(|p| p.listed_currency() != *currency)
        })
        .map(|currency| {
            let set = price
                .as_ref()
                .and_then(|p| p.currency_prices.iter().find(|m| m.currency == currency));
            (
                currency.to_owned(),
                set.map_or_else(|| "non vendu".to_owned(), money),
                set.map(|m| m.minor.to_string()).unwrap_or_default(),
            )
        })
        .collect();
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
    // The family the product is a variant of: its name, where it stands in
    // it, and the way there. A product that says it joined a family where it
    // has no place yet shows « à placer ».
    let variant_of: Option<(String, String, String)> = match &product.family_id {
        Some(family_id) => timada_catalog::Command(&services.executor)
            .load_family(family_id)
            .await?
            .map(|family| {
                let standing = family.variant(&id).map_or_else(
                    || "à placer".to_owned(),
                    |variant| {
                        variant
                            .values
                            .iter()
                            .map(|placed| format!("{} : {}", placed.option, placed.value))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    },
                );
                let link = href!(show_family, FamilyId(family.id.clone())).resolve(cx);
                (family.name, standing, link)
            }),
        None => None,
    };
    let sheet = product
        .specs
        .iter()
        .map(|spec| format!("{} | {} | {}", spec.group, spec.label, spec.value))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(view! {
        page_header(
            parent: Section::Products,
            title: &product.name,
            if product.archived { <span class="text-sm text-muted-foreground">"Archivé"</span> }
        )
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">
            (product.brand.name.clone()) " · " (product.sku.clone()) " · " (filed_under)
            " · garantie " (product.warranty_months.to_string()) " mois"
        </p>

        detail_grid(
            detail_main(
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
                card(
                    card_header(card_title("Fiche technique"))
                    card_content(
                        <form method="post" action=(href!(specify, ProductId(id.clone()))) class="flex flex-col gap-3">
                            label(attrs: topcoat::view::attributes! { for="specs" }, "Une ligne par caractéristique : Groupe | Libellé | Valeur")
                            textarea(attrs: topcoat::view::attributes! { id="specs" name="specs" rows="8" placeholder="Dalle | Taille | 27 pouces" }, (sheet.clone()))
                            <p class="text-sm text-muted-foreground">"Les filtres des catégories s'appuient sur ces lignes : une même valeur s'écrit partout de la même façon."</p>
                            if !product.archived {
                                <div>button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer la fiche")</div>
                            }
                        </form>
                    )
                )
            )
            detail_main(
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
                                    if !other_prices.is_empty() {
                                        <h3 class="mt-4 text-sm font-medium">"Autres devises"</h3>
                                        <p class="mb-2 text-xs text-muted-foreground">"Un prix décidé par devise, jamais converti. Sans prix, le produit n'est pas vendu dans cette devise ; laissez vide pour le retirer."</p>
                                        for (currency, current, cents) in &other_prices {
                                            <form method="post" action=(href!(set_currency_price, ProductId(id.clone()))) class="mb-2 flex items-center gap-2 text-sm">
                                                <input type="hidden" name="currency" value=(currency.clone())>
                                                <span class="w-24 tabular-nums">(current.clone())</span>
                                                input(attrs: topcoat::view::attributes! { name="price_cents" type="number" min="1" value=(cents.clone()) placeholder=(format!("Prix TTC en {currency} (centimes)")) aria-label=(format!("Prix TTC en {currency}, en centimes")) })
                                                button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer")
                                            </form>
                                        }
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
                    card(
                        card_header(card_title("Famille"))
                        card_content(
                            if let Some((family_name, standing, link)) = &variant_of {
                                <p class="text-sm">"Variante de " <a href=(link.clone()) class="underline underline-offset-4">(family_name.clone())</a></p>
                                <p class="mb-3 text-sm text-muted-foreground">(standing.clone())</p>
                                <form method="post" action=(href!(leave_family, ProductId(id.clone())))>
                                    button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Retirer de la famille")
                                </form>
                            } else {
                                <p class="text-sm text-muted-foreground">"Ce produit n'est la variante d'aucune famille. Il se place depuis la section Familles, par sa référence."</p>
                            }
                        )
                    )
                    <form method="post" action=(href!(archive, ProductId(id.clone())))>
                        button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Archiver le produit")
                    </form>
                }
            )
        )
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
    let currency = listed_currency(cx, &id).await?;
    timada_pricing::Command(&services.executor)
        .change_price(price_id(&id), Money::new(form.price_cents, currency))
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

/// The currency the product's price was listed in — the shop's base currency
/// for a product without a price.
async fn listed_currency(cx: &Cx, product_id: &str) -> Result<String> {
    let services = app_context::<AdminServices>(cx);
    Ok(load_product_price(&services.executor, price_id(product_id))
        .await?
        .map(|price| price.listed_currency().to_owned())
        .unwrap_or_else(|| app_context::<AdminConfig>(cx).currencies.base().to_owned()))
}

#[derive(Debug, Deserialize)]
pub struct CurrencyPriceForm {
    currency: String,
    /// Empty: the product is no longer sold in that currency.
    price_cents: Option<String>,
}

/// Sets — or, left empty, removes — the product's price in one of the shop's
/// other currencies.
#[page(POST "./currency-price")]
pub async fn set_currency_price(cx: &Cx, Form(form): Form<CurrencyPriceForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let currencies = &app_context::<AdminConfig>(cx).currencies;
    // Only the currencies the shop sells in: a price elsewhere would never
    // be shown.
    if !currencies.others().contains(&form.currency) {
        return Err(topcoat::router::error::bad_request(format!(
            "the shop does not sell in {}",
            form.currency
        ))
        .into());
    }
    let pricing = timada_pricing::Command(&services.executor);
    match form
        .price_cents
        .as_deref()
        .map(str::trim)
        .filter(|cents| !cents.is_empty())
    {
        Some(cents) => {
            let Ok(minor) = cents.parse::<i64>() else {
                return Err(topcoat::router::error::bad_request(format!(
                    "`{cents}` is not an amount in cents"
                ))
                .into());
            };
            pricing
                .set_currency_price(price_id(&id), Money::new(minor, &form.currency))
                .await?;
        }
        None => {
            pricing
                .remove_currency_price(price_id(&id), &form.currency)
                .await?;
        }
    }
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
    let currency = listed_currency(cx, &id).await?;
    timada_pricing::Command(&services.executor)
        .attach_installment_offer(
            price_id(&id),
            InstallmentOffer {
                count: 3,
                fee: Money::new(form.fee_cents, currency),
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

/// Takes the product out of the family it says it is in — also when the
/// family never recorded its place.
#[page(POST "./leave-family")]
pub async fn leave_family(cx: &Cx) -> Result<impl View> {
    let (id, product) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    if let Some(family_id) = &product.family_id {
        timada_catalog::Command(&services.executor)
            .remove_variant(family_id, &id)
            .await?;
    }
    Err::<(), _>(see_other(back(cx, &id)).into())
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

#[derive(Debug, Deserialize)]
pub struct SpecifyForm {
    specs: String,
}

/// Replaces the technical sheet: `Groupe | Libellé | Valeur` per line, the
/// group optional; lines without a label or a value are dropped.
#[page(POST "./specify")]
pub async fn specify(cx: &Cx, Form(form): Form<SpecifyForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let specs = form
        .specs
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('|').map(str::trim).collect();
            let (group, name, value) = match parts.as_slice() {
                [group, name, value] => (*group, *name, *value),
                [name, value] => ("", *name, *value),
                _ => return None,
            };
            (!name.is_empty() && !value.is_empty()).then(|| Spec {
                group: group.to_owned(),
                label: name.to_owned(),
                value: value.to_owned(),
            })
        })
        .collect();
    timada_catalog::Command(&services.executor)
        .specify_product(&id, specs)
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}
