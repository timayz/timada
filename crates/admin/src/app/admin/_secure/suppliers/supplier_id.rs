//! `/{mount}/suppliers/{supplier_id}`: one supplier — the terms its products
//! are priced on, whether it is bought from at all, and what it sources.

use serde::Deserialize;
use timada_core::Money;
use timada_sourcing::{
    PricingRule, RuleScope, count_sourced_of_supplier, hurry_supplier, resolve_rule,
    sourced_of_supplier, supplier_by_id,
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

use crate::{
    app::admin::_secure::products::product_id::{ProductId, show as show_product},
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{detail_grid, detail_main, empty_state, link, page_header},
};

path_param!(pub supplier_id: String, error = not_found);

const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct ShowQuery {
    page: Option<u32>,
    /// A short code, not a sentence: what it says is written here, so a URL
    /// can never put words in the shop's mouth.
    done: Option<String>,
    n: Option<u64>,
}

fn done_message(code: &str, n: Option<u64>) -> Option<String> {
    match code {
        "rule" => Some("Terme enregistré.".to_owned()),
        "rule-cleared" => Some("Terme de la boutique rétabli.".to_owned()),
        "suspended" => Some("Fournisseur suspendu.".to_owned()),
        "resumed" => Some("Fournisseur réactivé.".to_owned()),
        "queued" => Some(format!(
            "{} produit(s) en tête de file : la prochaine passe s'en occupe.",
            n.unwrap_or_default()
        )),
        _ => None,
    }
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let id = param::<SupplierId>(cx)?.clone();
    let query = query::<ShowQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let services = app_context::<AdminServices>(cx);
    let Some(supplier) = supplier_by_id(&services.db, &id).await? else {
        return Err(topcoat::router::error::not_found().into());
    };
    let rule = resolve_rule(&services.db, &id, "").await?;
    let own = timada_sourcing::load_rule(&services.db, &RuleScope::Supplier(id.clone())).await?;
    let rows = sourced_of_supplier(&services.db, &id, PAGE_SIZE, (page - 1) * PAGE_SIZE).await?;
    let total = count_sourced_of_supplier(&services.db, &id).await?;
    let can_be_asked = services
        .suppliers
        .as_ref()
        .and_then(|connectors| connectors.of(&supplier.connector))
        .is_some_and(|connector| connector.does(timada_sourcing::ConnectorTask::Offers));

    let title = supplier.name.clone();
    Ok(view! {
        page_header(parent: crate::auth::Section::Suppliers, title: &title)
        if let Some(message) = query.done.as_deref().and_then(|code| done_message(code, query.n)) {
            <p class="mb-4 rounded-md border border-border bg-muted px-3 py-2 text-sm">(message)</p>
        }
        detail_grid(
            detail_main(
                card(
                    card_header(card_title("Terme d'achat"))
                    card_content(
                        <form method="post" action=(href!(set_rule, SupplierId(id.clone())).resolve(cx)) class="grid gap-4 sm:grid-cols-2">
                            rule_field(name: "markup_bp", text: "Marge sur le coût (points de base)", value: rule.markup_bp.to_string())
                            rule_field(name: "min_margin_bp", text: "Marge minimale (points de base)", value: rule.min_margin_bp.to_string())
                            rule_field(name: "auto_move_bp", text: "Écart accepté sans validation (points de base)", value: rule.auto_move_bp.to_string())
                            rule_field(name: "auto_move_cap_minor", text: "… et au plus, en centimes", value: rule.auto_move_cap.minor.to_string())
                            rule_field(name: "round_step_minor", text: "Arrondi au multiple de (centimes)", value: rule.round_step_minor.to_string())
                            rule_field(name: "round_ends_minor", text: "… se terminant par (centimes)", value: rule.round_ends_minor.to_string())
                            rule_field(name: "safety_stock", text: "Unités gardées en réserve", value: rule.safety_stock.to_string())
                            <div class="flex items-end gap-4">
                                <label class="flex items-center gap-2 text-sm">
                                    <input type="checkbox" name="shipping_included" checked=(rule.shipping_included)>
                                    "Port compris"
                                </label>
                                <label class="flex items-center gap-2 text-sm">
                                    <input type="checkbox" name="eco_on_top" checked=(rule.eco_on_top)>
                                    "Éco-part. en sus"
                                </label>
                            </div>
                            <p class="sm:col-span-2 text-xs text-muted-foreground">
                                if own.is_some() {
                                    "Ce fournisseur a son propre terme ; sans lui, celui de la boutique s'applique."
                                } else {
                                    "Ce fournisseur suit le terme de la boutique. Enregistrer lui en donne un."
                                }
                            </p>
                            <div class="sm:col-span-2 flex gap-2">
                                button(attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer")
                            </div>
                        </form>
                        if own.is_some() {
                            <form method="post" action=(href!(clear_rule, SupplierId(id.clone())).resolve(cx)) class="mt-2">
                                button(variant: ButtonVariant::Ghost, attrs: topcoat::view::attributes! { type="submit" }, "Revenir au terme de la boutique")
                            </form>
                        }
                    )
                )
                <div class="mt-6">
                    if rows.is_empty() {
                        empty_state(message: "Ce fournisseur n'approvisionne aucun produit.")
                    } else {
                        table(
                            table_header(table_row(
                                table_head("Produit") table_head("Article") table_head("Prix")
                            ))
                            table_body(
                                for row in &rows {
                                    table_row(
                                        table_cell(
                                            link(
                                                href: href!(show_product, ProductId(row.product_id.clone())).resolve(cx),
                                                (row.product_id.clone())
                                            )
                                        )
                                        table_cell(
                                            <span class="font-mono text-xs">(row.external_item_id.clone())</span>
                                            if let Some(sku) = &row.external_sku {
                                                <span class="ml-1 font-mono text-xs text-muted-foreground">(sku.clone())</span>
                                            }
                                        )
                                        table_cell(
                                            if row.locked {
                                                badge(variant: BadgeVariant::Secondary, "Verrouillé")
                                            } else {
                                                badge(variant: BadgeVariant::Outline, "Suivi")
                                            }
                                        )
                                    )
                                }
                            )
                        )
                        <p class="mt-2 text-xs text-muted-foreground">(format!("{total} produit(s)"))</p>
                    }
                </div>
            )
            <div class="grid gap-6">
                card(
                    card_header(card_title("État"))
                    card_content(
                        <dl class="grid gap-2 text-sm">
                            <div class="flex justify-between"><dt class="text-muted-foreground">"Connecteur"</dt><dd class="font-mono text-xs">(supplier.connector.clone())</dd></div>
                            <div class="flex justify-between"><dt class="text-muted-foreground">"Devise"</dt><dd>(supplier.currency.clone())</dd></div>
                        </dl>
                        if !can_be_asked {
                            <p class="mt-3 text-xs text-muted-foreground">
                                "Aucun connecteur ne répond pour ce fournisseur : les coûts et les niveaux se saisissent à la main."
                            </p>
                        }
                        <div class="mt-4 flex flex-wrap gap-2">
                            if supplier.suspended {
                                <form method="post" action=(href!(resume, SupplierId(id.clone())).resolve(cx))>
                                    button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Reprendre")
                                </form>
                            } else {
                                <form method="post" action=(href!(suspend, SupplierId(id.clone())).resolve(cx))>
                                    button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Suspendre")
                                </form>
                            }
                            if can_be_asked && !supplier.suspended {
                                <form method="post" action=(href!(sync_now, SupplierId(id.clone())).resolve(cx))>
                                    button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Synchroniser maintenant")
                                </form>
                            }
                        </div>
                        if let Some(reason) = &supplier.suspended_reason {
                            <p class="mt-3 text-xs text-muted-foreground">(reason.clone())</p>
                        }
                    )
                )
            </div>
        )
    })
}

#[topcoat::view::component]
async fn rule_field(name: &str, text: &str, value: String) -> Result<impl View> {
    Ok(view! {
        <div class="grid gap-2">
            label(attrs: topcoat::view::attributes! { for=(name) }, (text.to_owned()))
            input(attrs: topcoat::view::attributes! { id=(name) name=(name) type="number" min="0" required=(true) value=(value) })
        </div>
    })
}

#[derive(Debug, Deserialize)]
pub struct RuleForm {
    markup_bp: u16,
    min_margin_bp: u16,
    auto_move_bp: u16,
    auto_move_cap_minor: i64,
    round_step_minor: i64,
    round_ends_minor: i64,
    safety_stock: u32,
    shipping_included: Option<String>,
    eco_on_top: Option<String>,
}

#[page(POST "./rule")]
pub async fn set_rule(cx: &Cx, Form(form): Form<RuleForm>) -> Result<impl View> {
    let id = param::<SupplierId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let Some(supplier) = supplier_by_id(&services.db, &id).await? else {
        return Err(topcoat::router::error::not_found().into());
    };
    // The cap is compared with a selling price, so it is in the shop's own
    // currency, not the supplier's: a cap nothing can be compared with would
    // send every change to the queue.
    let currency = timada_core::ShopCurrencies::default().base().to_owned();
    let _ = &supplier;
    timada_sourcing::save_rule(
        &services.db,
        &RuleScope::Supplier(id.clone()),
        &PricingRule {
            markup_bp: form.markup_bp,
            min_margin_bp: form.min_margin_bp,
            auto_move_bp: form.auto_move_bp,
            auto_move_cap: Money::new(form.auto_move_cap_minor.max(0), currency),
            round_step_minor: form.round_step_minor.max(1),
            round_ends_minor: form.round_ends_minor.max(0),
            safety_stock: form.safety_stock,
            shipping_included: form.shipping_included.is_some(),
            eco_on_top: form.eco_on_top.is_some(),
        },
    )
    .await?;
    Err::<(), _>(see_other(back(cx, &id, "rule")).into())
}

#[page(POST "./rule/clear")]
pub async fn clear_rule(cx: &Cx) -> Result<impl View> {
    let id = param::<SupplierId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    timada_sourcing::clear_rule(&services.db, &RuleScope::Supplier(id.clone())).await?;
    Err::<(), _>(see_other(back(cx, &id, "rule-cleared")).into())
}

#[page(POST "./suspend")]
pub async fn suspend(cx: &Cx) -> Result<impl View> {
    let id = param::<SupplierId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    timada_sourcing::Command::new(&services.executor, services.db.clone())
        .suspend_supplier(&id, "suspendu depuis le back-office".into())
        .await?;
    Err::<(), _>(see_other(back(cx, &id, "suspended")).into())
}

#[page(POST "./resume")]
pub async fn resume(cx: &Cx) -> Result<impl View> {
    let id = param::<SupplierId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    timada_sourcing::Command::new(&services.executor, services.db.clone())
        .resume_supplier(&id)
        .await?;
    Err::<(), _>(see_other(back(cx, &id, "resumed")).into())
}

/// Puts everything this supplier sources at the head of the queue. It does
/// **not** call the supplier here: a page that waits on somebody else's API
/// is a page that times out.
#[page(POST "./sync")]
pub async fn sync_now(cx: &Cx) -> Result<impl View> {
    let id = param::<SupplierId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let queued = hurry_supplier(&services.db, &id, 0).await?;
    Err::<(), _>(see_other(format!("{}&n={queued}", back(cx, &id, "queued"))).into())
}

fn back(cx: &Cx, id: &str, done: &str) -> String {
    let path = href!(show, SupplierId(id.to_owned())).resolve(cx);
    format!("{path}?done={done}")
}
