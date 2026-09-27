//! `/{mount}/purchasing`: « Achats » — what the shop has to buy from its
//! suppliers for the orders it has taken, and what became of each purchase.
//!
//! Opened by the books' roles and by customer service, who answer « où est ma
//! commande ? »; but only a role that may move money **places** one, because
//! that is the shop's cash going out.

use std::collections::HashMap;

use timada_sourcing::{
    ListPurchases, PurchaseRow, SupplierOrderStatus, count_purchases, list_purchases,
    list_suppliers,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, view},
};

use crate::{
    app::admin::_secure::{
        orders::order_id::{OrderId, show as show_order},
        suppliers::supplier_href,
    },
    components::{
        badge::{BadgeVariant, badge},
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date, empty_state, filter_bar, link, money, page_header, pagination, table_card},
};

pub mod purchase_id;

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct PurchasingQuery {
    page: Option<u32>,
    /// A status, or `all`. Defaults to what is still to be bought.
    etat: Option<String>,
    error: Option<String>,
}

/// How a purchase's standing reads at a glance.
pub fn purchase_status_badge_variant(status: SupplierOrderStatus) -> BadgeVariant {
    match status {
        SupplierOrderStatus::Drafted => BadgeVariant::Secondary,
        SupplierOrderStatus::Placed => BadgeVariant::Info,
        SupplierOrderStatus::Shipped => BadgeVariant::Success,
        SupplierOrderStatus::Refused => BadgeVariant::Destructive,
        SupplierOrderStatus::Cancelled => BadgeVariant::Outline,
    }
}

/// What a refusal on one of these pages means.
pub fn refusal(code: &str) -> &'static str {
    match code {
        "state" => "Cet achat n'en est plus là : rechargez la page.",
        "shipped" => "Le colis est déjà parti : la voie de retour est un retour.",
        "reference" => "Indiquez la référence de la commande chez le fournisseur.",
        "forbidden" => {
            "Commander chez un fournisseur engage l'argent de la boutique : demandez à la comptabilité."
        }
        _ => "Action impossible.",
    }
}

struct Line {
    purchase: PurchaseRow,
    supplier_name: String,
    supplier_link: String,
    order_link: String,
    detail_link: String,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<PurchasingQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let wanted = query.etat.as_deref().unwrap_or("drafted");
    let services = app_context::<AdminServices>(cx);
    let filter = ListPurchases {
        status: SupplierOrderStatus::parse(wanted),
        supplier_id: None,
        limit: PAGE_SIZE,
        offset: (page - 1) * PAGE_SIZE,
    };
    let purchases = list_purchases(&services.db, &filter).await?;
    let total = count_purchases(&services.db, &filter).await?;
    let names: HashMap<String, String> = list_suppliers(&services.db)
        .await?
        .into_iter()
        .map(|supplier| (supplier.supplier_id, supplier.name))
        .collect();
    let lines: Vec<Line> = purchases
        .into_iter()
        .map(|purchase| Line {
            supplier_name: names
                .get(&purchase.supplier_id)
                .cloned()
                .unwrap_or_else(|| purchase.supplier_id.clone()),
            supplier_link: supplier_href(cx, &purchase.supplier_id),
            order_link: href!(show_order, OrderId(purchase.order_id.clone())).resolve(cx),
            detail_link: href!(
                purchase_id::show,
                purchase_id::PurchaseId(purchase.purchase_id.clone())
            )
            .resolve(cx),
            purchase,
        })
        .collect();

    Ok(view! {
        page_header(
            title: "Achats fournisseur",
            filter_bar(
                for (code, label) in tabs() {
                    <a href=(tab_href(cx, code)) class=(tab_class(code == wanted))>(label)</a>
                }
            )
        )
        if let Some(error) = &query.error {
            <p class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
                (refusal(error))
            </p>
        }
        if lines.is_empty() {
            empty_state(message: "Rien ici.")
        } else {
            table_card(
                table(
                    table_header(table_row(
                        table_head("Commande") table_head("Fournisseur")
                        table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Unités")
                        table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Coût")
                        table_head("État") table_head("Suivi") table_head("Du")
                    ))
                    table_body(for line in &lines { purchase_row(line: line) })
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

fn tabs() -> [(&'static str, &'static str); 5] {
    [
        ("drafted", "À commander"),
        ("placed", "Commandées"),
        ("shipped", "Expédiées"),
        ("refused", "Refusées"),
        ("all", "Toutes"),
    ]
}

fn tab_href(cx: &Cx, code: &str) -> String {
    format!("{}?etat={code}", href!(index).resolve(cx))
}

fn tab_class(active: bool) -> &'static str {
    if active {
        "rounded-md bg-muted px-3 py-1.5 text-sm font-medium"
    } else {
        "rounded-md px-3 py-1.5 text-sm text-muted-foreground hover:bg-muted"
    }
}

#[topcoat::view::component]
async fn purchase_row(line: &Line) -> Result<impl View> {
    let purchase = &line.purchase;
    Ok(view! {
        table_row(
            table_cell(
                link(href: line.detail_link.clone(), (purchase.units.to_string()) " article(s)")
                <a href=(line.order_link.clone()) class="ml-2 text-xs text-muted-foreground underline">"commande"</a>
            )
            table_cell(
                <a href=(line.supplier_link.clone()) class="underline underline-offset-4">(line.supplier_name.clone())</a>
            )
            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (purchase.units.to_string()))
            table_cell(
                attrs: topcoat::view::attributes! { class="text-right tabular-nums" },
                (money(purchase.charged.as_ref().unwrap_or(&purchase.cost)))
            )
            table_cell(
                badge(variant: purchase_status_badge_variant(purchase.status), (purchase.status.label()))
            )
            table_cell(
                if let Some(tracking) = &purchase.tracking_number {
                    <span class="font-mono text-xs">(tracking.clone())</span>
                } else if let Some(external) = &purchase.external_order_id {
                    <span class="font-mono text-xs text-muted-foreground">(external.clone())</span>
                } else { "—" }
            )
            table_cell((date(purchase.drafted_at as u64)))
        )
    })
}
