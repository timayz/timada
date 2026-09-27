//! `/{mount}/suppliers`: who the shop buys from, and on what terms.

use serde::Deserialize;
use timada_sourcing::{RegisterSupplier, SupplierConnectors, list_suppliers};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page},
    view::{View, view},
};

use crate::{
    components::{
        badge::{BadgeVariant, badge},
        button::button,
        card::{card, card_content},
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date, empty_state, form_error, link, page_header, table_card, text_field},
};

pub mod supplier_id;

/// What the connectors offer, for the « Connecteur » field. A shop with none
/// plugged in still takes suppliers on: they are worked by hand.
fn connector_keys(services: &AdminServices) -> Vec<String> {
    let mut keys: Vec<String> = services
        .suppliers
        .as_ref()
        .map(|connectors| connectors.keys().map(str::to_owned).collect())
        .unwrap_or_default();
    if keys.is_empty() {
        keys.push(timada_sourcing::ManualConnector::KEY.to_owned());
    }
    keys
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let rows = list_suppliers(&services.db).await?;
    let connectors = connector_keys(services);

    Ok(view! {
        page_header(title: "Fournisseurs")
        if rows.is_empty() {
            empty_state(message: "Aucun fournisseur. Ajoutez-en un pour approvisionner des produits.")
        } else {
            table_card(
                table(
                    table_header(table_row(
                        table_head("Nom") table_head("Connecteur")
                        table_head("Devise") table_head("État") table_head("Depuis")
                    ))
                    table_body(
                        for row in &rows {
                            table_row(
                                table_cell(
                                    link(
                                        href: href!(supplier_id::show, supplier_id::SupplierId(row.supplier_id.clone())).resolve(cx),
                                        (row.name.clone())
                                    )
                                    <span class="ml-2 font-mono text-xs text-muted-foreground">(row.slug.clone())</span>
                                )
                                table_cell(<span class="font-mono text-xs">(row.connector.clone())</span>)
                                table_cell((row.currency.clone()))
                                table_cell(
                                    if row.suspended {
                                        badge(variant: BadgeVariant::Destructive, "Suspendu")
                                    } else {
                                        badge(variant: BadgeVariant::Secondary, "Actif")
                                    }
                                )
                                table_cell((date(row.registered_at as u64)))
                            )
                        }
                    )
                )
            )
        }
        <div class="mt-6 max-w-2xl">
            new_supplier_form(connectors: connectors, error: None)
        </div>
    })
}

#[derive(Debug, Deserialize)]
pub struct NewSupplierForm {
    name: String,
    slug: String,
    connector: String,
    currency: String,
}

#[page(POST "./new")]
pub async fn create(cx: &Cx, Form(form): Form<NewSupplierForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let connectors = services
        .suppliers
        .clone()
        .unwrap_or_else(|| SupplierConnectors::default().with(timada_sourcing::ManualConnector));
    let outcome = timada_sourcing::Command::new(&services.executor, services.db.clone())
        .register_supplier(
            RegisterSupplier {
                slug: if form.slug.trim().is_empty() {
                    form.name.clone()
                } else {
                    form.slug.clone()
                },
                name: form.name.clone(),
                connector: form.connector.clone(),
                currency: form.currency.clone(),
            },
            &connectors,
        )
        .await;
    match outcome {
        Ok(_) => Err(see_other(href!(index).resolve(cx)).into()),
        Err(error) => Ok(view! {
            page_header(title: "Fournisseurs")
            <div class="max-w-2xl">
                new_supplier_form(connectors: connector_keys(services), error: Some(error.to_string()))
            </div>
        }),
    }
}

#[topcoat::view::component]
async fn new_supplier_form(
    cx: &Cx,
    connectors: Vec<String>,
    error: Option<String>,
) -> Result<impl View> {
    Ok(view! {
        card(card_content(
            <form method="post" action=(href!(create).resolve(cx)) class="grid gap-4 sm:grid-cols-2">
                <h2 class="sm:col-span-2 text-sm font-semibold">"Ajouter un fournisseur"</h2>
                text_field(name: "name", label_text: "Nom", attrs: topcoat::view::attributes! { required=(true) autocomplete="off" })
                text_field(name: "slug", label_text: "Identifiant (vide = d'après le nom)", attrs: topcoat::view::attributes! { autocomplete="off" })
                <div class="grid gap-2">
                    <label for="connector" class="text-sm font-medium">"Connecteur"</label>
                    <select id="connector" name="connector" class="h-9 rounded-md border border-input bg-transparent px-3 text-sm">
                        for key in &connectors {
                            <option value=(key.clone())>(key.clone())</option>
                        }
                    </select>
                </div>
                text_field(name: "currency", label_text: "Devise des coûts", attrs: topcoat::view::attributes! { required=(true) value="USD" maxlength="3" autocomplete="off" })
                if let Some(error) = &error {
                    form_error(class: "sm:col-span-2", (error.clone()))
                }
                <div class="sm:col-span-2">
                    button(attrs: topcoat::view::attributes! { type="submit" }, "Ajouter")
                </div>
            </form>
        ))
    })
}

/// Reused by the product page's « Approvisionnement » card.
pub fn supplier_href(cx: &Cx, supplier_id: &str) -> String {
    href!(
        supplier_id::show,
        supplier_id::SupplierId(supplier_id.to_owned())
    )
    .resolve(cx)
}
