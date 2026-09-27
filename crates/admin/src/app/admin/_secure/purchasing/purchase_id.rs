//! `/{mount}/purchasing/{purchase_id}`: one purchase — what it is for, what it
//! comes to, and the three things an operator can do about it.
//!
//! Placing and cancelling spend or unspend the shop's money, so they are the
//! books' roles'. Customer service sees the page and the tracking without
//! them.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::products_by_ids;
use timada_sourcing::{
    SourcingError, SupplierOrderStatus, enqueue_place, purchase_by_id, supplier_by_id,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::see_other, href, page, path_param, path_param as param, query_params,
        query_params as query,
    },
    view::{View, view},
};

use super::{purchase_status_badge_variant, refusal};
use crate::{
    app::admin::_secure::{
        orders::order_id::{OrderId, show as show_order},
        suppliers::supplier_href,
    },
    auth::Section,
    components::{
        badge::badge,
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{detail_grid, detail_main, money, page_header, text_field},
};

path_param!(pub purchase_id: String, error = not_found);

#[query_params(error = bad_request)]
struct ShowQuery {
    error: Option<String>,
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let id = param::<PurchaseId>(cx)?.clone();
    let query = query::<ShowQuery>(cx)?;
    let services = app_context::<AdminServices>(cx);
    let Some(purchase) = purchase_by_id(&services.db, &id).await? else {
        return Err(topcoat::router::error::not_found().into());
    };
    let view = timada_sourcing::load_supplier_order(&services.executor, &id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let supplier = supplier_by_id(&services.db, &purchase.supplier_id).await?;
    let lines = view
        .as_ref()
        .map(|view| view.lines.clone())
        .unwrap_or_default();
    let product_ids: Vec<String> = lines.iter().map(|line| line.product_id.clone()).collect();
    let names: HashMap<String, String> = products_by_ids(&services.db, &product_ids)
        .await?
        .into_iter()
        .map(|product| (product.id, product.name))
        .collect();

    let moves_money = crate::auth::signed_in_admin(cx)
        .map(|admin| admin.role)
        .is_some_and(|role| role.moves_money());
    let can_order = moves_money && purchase.status == SupplierOrderStatus::Drafted;
    let can_record = moves_money
        && matches!(
            purchase.status,
            SupplierOrderStatus::Drafted | SupplierOrderStatus::Refused
        );
    let can_cancel = moves_money && purchase.status.is_open();
    let overrun = view.as_ref().and_then(|view| view.overrun());
    let title = format!("Achat · {}", supplier.as_ref().map_or("—", |s| &s.name));

    Ok(view! {
        page_header(parent: Section::Purchasing, title: &title)
        if let Some(error) = &query.error {
            <p class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
                (refusal(error))
            </p>
        }
        detail_grid(
            detail_main(
                card(
                    card_header(card_title("Ce qu'il faut acheter"))
                    card_content(
                        table(
                            table_header(table_row(
                                table_head("Produit") table_head("Article chez le fournisseur")
                                table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Qté")
                                table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Coût unitaire")
                            ))
                            table_body(
                                for line in &lines {
                                    table_row(
                                        table_cell((names.get(&line.product_id).cloned().unwrap_or_else(|| line.product_id.clone())))
                                        table_cell(
                                            <span class="font-mono text-xs">(line.external_item_id.clone())</span>
                                            if let Some(sku) = &line.external_sku {
                                                <span class="ml-1 font-mono text-xs text-muted-foreground">(sku.clone())</span>
                                            }
                                        )
                                        table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (line.quantity.to_string()))
                                        table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (money(&line.unit_cost)))
                                    )
                                }
                            )
                        )
                    )
                )
                if let Some(address) = view.as_ref().map(|view| view.ship_to.clone()) {
                    <div class="mt-6">
                        card(
                            card_header(card_title("Livrer à"))
                            card_content(
                                <address class="text-sm not-italic leading-relaxed">
                                    (format!("{} {}", address.first_name, address.last_name)) <br>
                                    (address.line1.clone()) <br>
                                    if let Some(line2) = &address.line2 {
                                        (line2.clone()) <br>
                                    }
                                    (format!("{} {}", address.postal_code, address.city)) <br>
                                    (address.country_code.clone())
                                </address>
                            )
                        )
                    </div>
                }
                if can_record {
                    <div class="mt-6">
                        card(
                            card_header(card_title("Commandé à la main"))
                            card_content(
                                <p class="mb-4 text-sm text-muted-foreground">
                                    "Acheté sur le site du fournisseur : indiquez la référence de sa commande pour que le suivi retombe sur celle du client."
                                </p>
                                <form method="post" action=(href!(record_by_hand, PurchaseId(id.clone())).resolve(cx)) class="grid gap-4 sm:grid-cols-2">
                                    text_field(name: "external_order_id", label_text: "Référence chez le fournisseur", attrs: topcoat::view::attributes! { required=(true) autocomplete="off" })
                                    text_field(name: "note", label_text: "Note (facultatif)", attrs: topcoat::view::attributes! { autocomplete="off" })
                                    <div class="sm:col-span-2">
                                        button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer la référence")
                                    </div>
                                </form>
                            )
                        )
                    </div>
                }
            )
            <div class="grid gap-6">
                card(
                    card_header(card_title("État"))
                    card_content(
                        <dl class="grid gap-2 text-sm">
                            <div class="flex justify-between">
                                <dt class="text-muted-foreground">"Standing"</dt>
                                <dd>badge(variant: purchase_status_badge_variant(purchase.status), (purchase.status.label()))</dd>
                            </div>
                            <div class="flex justify-between">
                                <dt class="text-muted-foreground">"Devis"</dt>
                                <dd class="tabular-nums">(money(&purchase.cost))</dd>
                            </div>
                            if let Some(charged) = &purchase.charged {
                                <div class="flex justify-between">
                                    <dt class="text-muted-foreground">"Facturé"</dt>
                                    <dd class="tabular-nums">(money(charged))</dd>
                                </div>
                            }
                            if let Some(external) = &purchase.external_order_id {
                                <div class="flex justify-between">
                                    <dt class="text-muted-foreground">"Référence"</dt>
                                    <dd class="font-mono text-xs">(external.clone())</dd>
                                </div>
                            }
                            if let Some(tracking) = &purchase.tracking_number {
                                <div class="flex justify-between">
                                    <dt class="text-muted-foreground">"Suivi"</dt>
                                    <dd class="font-mono text-xs">(tracking.clone())</dd>
                                </div>
                            }
                        </dl>
                        if let Some(overrun) = &overrun {
                            <p class="mt-3 rounded-md border border-border bg-muted px-3 py-2 text-xs">
                                "Le fournisseur a facturé " (money(overrun)) " de plus que son devis."
                            </p>
                        }
                        if let Some(note) = &purchase.note {
                            <p class="mt-3 text-xs text-muted-foreground">(note.clone())</p>
                        }
                        <div class="mt-4 flex flex-col gap-2">
                            <a href=(href!(show_order, OrderId(purchase.order_id.clone())).resolve(cx)) class="text-sm underline underline-offset-4">"Voir la commande du client"</a>
                            <a href=(supplier_href(cx, &purchase.supplier_id)) class="text-sm underline underline-offset-4">"Voir le fournisseur"</a>
                        </div>
                        if can_order || can_cancel {
                            <div class="mt-4 flex flex-wrap gap-2">
                                if can_order {
                                    <form method="post" action=(href!(order_it, PurchaseId(id.clone())).resolve(cx))>
                                        button(attrs: topcoat::view::attributes! { type="submit" }, "Commander chez le fournisseur")
                                    </form>
                                }
                                if can_cancel {
                                    <form method="post" action=(href!(call_off, PurchaseId(id.clone())).resolve(cx))>
                                        button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" }, "Annuler l'achat")
                                    </form>
                                }
                            </div>
                        } else if !moves_money && purchase.status.is_open() {
                            <p class="mt-4 text-xs text-muted-foreground">
                                "Commander engage l'argent de la boutique : la comptabilité s'en charge."
                            </p>
                        }
                    )
                )
            </div>
        )
    })
}

/// Enqueues the purchase. The connector is **not** called here: a page that
/// waits on somebody else's API is a page that times out — the ticker does the
/// talking, and its failures land back on this page as a standing.
#[page(POST "./order")]
pub async fn order_it(cx: &Cx) -> Result<impl View> {
    let id = param::<PurchaseId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    if !moves_money(cx) {
        return Err(see_other(back(cx, &id, Some("forbidden"))).into());
    }
    let Some(purchase) = purchase_by_id(&services.db, &id).await? else {
        return Err(topcoat::router::error::not_found().into());
    };
    if purchase.status != SupplierOrderStatus::Drafted {
        return Err(see_other(back(cx, &id, Some("state"))).into());
    }
    enqueue_place(&services.db, &id).await?;
    Err::<(), _>(see_other(back(cx, &id, None)).into())
}

#[derive(Debug, Deserialize)]
pub struct ByHandForm {
    external_order_id: String,
    note: String,
}

#[page(POST "./by-hand")]
pub async fn record_by_hand(cx: &Cx, Form(form): Form<ByHandForm>) -> Result<impl View> {
    let id = param::<PurchaseId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    if !moves_money(cx) {
        return Err(see_other(back(cx, &id, Some("forbidden"))).into());
    }
    let done = timada_sourcing::Command::new(&services.executor, services.db.clone())
        .record_supplier_order_by_hand(&id, form.external_order_id, form.note)
        .await;
    Err::<(), _>(see_other(back(cx, &id, code(done))).into())
}

#[page(POST "./cancel")]
pub async fn call_off(cx: &Cx) -> Result<impl View> {
    let id = param::<PurchaseId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    if !moves_money(cx) {
        return Err(see_other(back(cx, &id, Some("forbidden"))).into());
    }
    let done = timada_sourcing::Command::new(&services.executor, services.db.clone())
        .cancel_supplier_order(&id, "annulé depuis le back-office".into())
        .await;
    Err::<(), _>(see_other(back(cx, &id, code(done))).into())
}

fn moves_money(cx: &Cx) -> bool {
    crate::auth::signed_in_admin(cx)
        .map(|admin| admin.role)
        .is_some_and(|role| role.moves_money())
}

fn code(done: std::result::Result<(), SourcingError>) -> Option<&'static str> {
    match done {
        Ok(()) => None,
        Err(SourcingError::SupplierOrderShipped) => Some("shipped"),
        Err(SourcingError::SupplierOrderNotDraft | SourcingError::SupplierOrderNotPlaced) => {
            Some("state")
        }
        Err(SourcingError::Required("external_order_id")) => Some("reference"),
        Err(err) => {
            tracing::error!(%err, "a purchase action failed");
            Some("server")
        }
    }
}

fn back(cx: &Cx, id: &str, error: Option<&str>) -> String {
    let path = href!(show, PurchaseId(id.to_owned())).resolve(cx);
    match error {
        Some(code) => format!("{path}?error={code}"),
        None => path,
    }
}
