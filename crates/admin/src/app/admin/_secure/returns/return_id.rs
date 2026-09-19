//! `/{mount}/returns/{return_id}`: one return — review the request, then
//! record the parcel: what is taken back, what goes into stock again, and how
//! the customer is refunded. The rest is the returns process manager's job.

use std::collections::HashMap;

use serde::Deserialize;
use timada_order::order_numbers_by_ids;
use timada_returns::{
    ReceiveReturn, ReceivedLine, RefundMethod, ReturnError, ReturnPolicy, ReturnStatus, ReturnView,
    load_return,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param, query_params, query_params as query,
    },
    view::{View, view},
};

use super::return_status_badge;
use crate::{
    app::admin::_secure::{customers::customer_id, orders::order_id},
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        separator::separator,
    },
    config::AdminServices,
    ui::{date, money, page_header},
};

path_param!(pub return_id: String, error = not_found);

/// Set by the actions when the returns context refuses them.
#[query_params(error = bad_request)]
struct ShowQuery {
    error: Option<String>,
}

fn error_message(code: Option<&str>) -> Option<&'static str> {
    match code? {
        "accepted" => Some("On ne peut pas reprendre plus d'articles que demandé."),
        "status" => Some("Ce retour n'est plus dans l'état attendu : la page a été rechargée."),
        "reason" => Some("Indiquez le motif du refus."),
        _ => None,
    }
}

fn returns(services: &AdminServices) -> timada_returns::Command<'_, evento::Evento> {
    timada_returns::Command {
        executor: &services.executor,
        db: services.db.clone(),
        policy: ReturnPolicy::default(),
    }
}

async fn load(cx: &Cx) -> Result<(String, ReturnView)> {
    let id = param::<ReturnId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let request = load_return(&services.executor, &id)
        .await?
        .ok_or_not_found()?;
    Ok((id, request))
}

fn back(cx: &Cx, id: &str) -> String {
    href!(show, ReturnId(id.to_owned())).resolve(cx)
}

/// Back to the return's page, with the refusal as a message when there is one.
fn settled(cx: &Cx, id: &str, outcome: std::result::Result<(), ReturnError>) -> Result<String> {
    let code = match outcome {
        Ok(()) => return Ok(back(cx, id)),
        Err(ReturnError::AcceptedExceedsRequested(_)) => "accepted",
        Err(ReturnError::WrongStatus { .. }) => "status",
        Err(ReturnError::Required(_)) => "reason",
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Ok(format!("{}?error={code}", back(cx, id)))
}

/// One requested line with what the parcel held, ready to render.
struct Line {
    product_field: String,
    accepted_field: String,
    restock_field: String,
    product_id: String,
    name: String,
    requested: String,
    unit_price: String,
    /// `(accepted, restocked)` once received.
    outcome: Option<(String, &'static str)>,
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let (id, request) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let error = error_message(query::<ShowQuery>(cx)?.error.as_deref());
    let order_label = order_numbers_by_ids(&services.db, std::slice::from_ref(&request.order_id))
        .await?
        .remove(&request.order_id)
        .unwrap_or_else(|| request.order_id.clone());
    let order_link = href!(order_id::show, order_id::OrderId(request.order_id.clone())).resolve(cx);
    let customer_link = href!(
        customer_id::show,
        customer_id::CustomerId(request.customer_id.clone())
    )
    .resolve(cx);

    let lines: Vec<Line> = request
        .lines
        .iter()
        .enumerate()
        .map(|(index, line)| Line {
            product_field: format!("product_{index}"),
            accepted_field: format!("accepted_{index}"),
            restock_field: format!("restock_{index}"),
            product_id: line.product_id.clone(),
            name: line.name.clone(),
            requested: line.quantity.to_string(),
            unit_price: money(&line.unit_price),
            outcome: request
                .received
                .iter()
                .find(|r| r.product_id == line.product_id)
                .map(|r| {
                    let restocked = if r.restock {
                        "remis en stock"
                    } else {
                        "non remis en stock"
                    };
                    (r.accepted.to_string(), restocked)
                }),
        })
        .collect();
    let title = format!("Retour {}", request.rma_number);
    let refund_method = request.refund_method.map(|method| match method {
        RefundMethod::OriginalPayment => "Moyen de paiement d'origine",
        RefundMethod::StoreCredit => "Avoir",
    });
    let voucher = request.voucher_code.clone();

    Ok(view! {
        page_header(
            title: &title,
            return_status_badge(status: request.status)
        )
        <p class="-mt-4 mb-6 font-mono text-xs text-muted-foreground">(id.clone()) " · demandé le " (date(request.requested_at))</p>
        if let Some(error) = error {
            <p role="alert" class="mb-4 text-sm text-destructive">(error)</p>
        }

        <div class="grid gap-6 lg:grid-cols-3">
            <div class="flex flex-col gap-6 lg:col-span-2">
                card(
                    card_header(card_title("Articles"))
                    card_content(
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="border-b border-border text-left text-muted-foreground">
                                    <th scope="col" class="py-2 font-normal">"Produit"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Prix payé"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Demandé"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Repris"</th>
                                </tr>
                            </thead>
                            <tbody>
                                for line in &lines {
                                    <tr class="border-b border-border last:border-0">
                                        <td class="py-2">(line.name.clone())</td>
                                        <td class="py-2 text-right tabular-nums">(line.unit_price.clone())</td>
                                        <td class="py-2 text-right tabular-nums">(line.requested.clone())</td>
                                        <td class="py-2 text-right tabular-nums">
                                            match &line.outcome {
                                                Some((accepted, restocked)) => { (accepted.clone()) <span class="ml-2 text-muted-foreground">(*restocked)</span> }
                                                None => { "—" }
                                            }
                                        </td>
                                    </tr>
                                }
                            </tbody>
                        </table>
                    )
                )
                if request.status == ReturnStatus::Approved {
                    card(
                        card_header(card_title("Réceptionner le colis"))
                        card_content(
                            <form method="post" action=(href!(receive_parcel, ReturnId(id.clone()))) class="flex flex-col gap-4 text-sm">
                                for line in &lines {
                                    <fieldset class="flex flex-wrap items-center gap-3">
                                        <legend class="font-medium">(line.name.clone())</legend>
                                        <input type="hidden" name=(line.product_field.clone()) value=(line.product_id.clone())>
                                        <label for=(line.accepted_field.clone()) class="text-muted-foreground">"Repris"</label>
                                        input(attrs: topcoat::view::attributes! { id=(line.accepted_field.clone()) name=(line.accepted_field.clone()) type="number" min="0" max=(line.requested.clone()) value=(line.requested.clone()) required=(true) class="w-24" })
                                        <label class="flex items-center gap-2">
                                            <input type="checkbox" name=(line.restock_field.clone()) value="on" checked=(true)>
                                            "Remettre en stock"
                                        </label>
                                    </fieldset>
                                }
                                <div class="flex flex-col gap-1">
                                    <label for="refund_method" class="text-muted-foreground">"Remboursement"</label>
                                    <select id="refund_method" name="refund_method" class="h-9 rounded-lg border border-border bg-background px-3">
                                        <option value="original" selected=(true)>"Moyen de paiement d'origine"</option>
                                        <option value="credit">"Avoir"</option>
                                    </select>
                                </div>
                                <div>
                                    button(attrs: topcoat::view::attributes! { type="submit" }, "Valider la réception")
                                </div>
                            </form>
                        )
                    )
                }
            </div>

            <div class="flex flex-col gap-6">
                card(
                    card_header(card_title("Demande"))
                    card_content(
                        <dl class="flex flex-col gap-2 text-sm">
                            <div><dt class="text-muted-foreground">"Commande"</dt><dd><a href=(order_link) class="font-mono text-xs underline-offset-4 hover:underline">(order_label)</a></dd></div>
                            <div><dt class="text-muted-foreground">"Client"</dt><dd><a href=(customer_link) class="font-mono text-xs underline-offset-4 hover:underline">(request.customer_id.clone())</a></dd></div>
                            <div><dt class="text-muted-foreground">"Motif"</dt><dd>(request.reason.clone())</dd></div>
                            if let Some(reason) = &request.refused_reason {
                                <div><dt class="text-muted-foreground">"Motif du refus"</dt><dd>(reason.clone())</dd></div>
                            }
                            if let Some(method) = refund_method {
                                <div><dt class="text-muted-foreground">"Remboursement"</dt><dd>(method)</dd></div>
                                <div><dt class="text-muted-foreground">"Sur le paiement"</dt><dd class="tabular-nums">(money(&request.money))</dd></div>
                                <div><dt class="text-muted-foreground">"En avoir"</dt><dd class="tabular-nums">(money(&request.credit))</dd></div>
                            }
                            if let Some(code) = &voucher {
                                <div><dt class="text-muted-foreground">"Code de l'avoir"</dt><dd class="font-mono text-xs">(code.clone())</dd></div>
                            }
                        </dl>
                    )
                )
                if request.status == ReturnStatus::Requested {
                    card(
                        card_header(card_title("Décision"))
                        card_content(
                            <div class="flex flex-col gap-3">
                                <form method="post" action=(href!(approve, ReturnId(id.clone())))>
                                    button(attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Accepter le retour")
                                </form>
                                separator()
                                <form method="post" action=(href!(refuse, ReturnId(id.clone()))) class="flex flex-col gap-2">
                                    input(attrs: topcoat::view::attributes! { name="reason" placeholder="Motif du refus" aria-label="Motif du refus" required=(true) })
                                    button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Refuser le retour")
                                </form>
                            </div>
                        )
                    )
                }
            </div>
        </div>
    })
}

#[page(POST "./approve")]
pub async fn approve(cx: &Cx) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let outcome = returns(services).approve_return(&id).await;
    Err::<(), _>(see_other(settled(cx, &id, outcome)?).into())
}

#[derive(Debug, Deserialize)]
pub struct RefuseForm {
    reason: String,
}

#[page(POST "./refuse")]
pub async fn refuse(cx: &Cx, Form(form): Form<RefuseForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let outcome = returns(services).refuse_return(&id, form.reason).await;
    Err::<(), _>(see_other(settled(cx, &id, outcome)?).into())
}

/// Records the parcel from the `product_<i>` / `accepted_<i>` / `restock_<i>`
/// fields; an unchecked box is simply absent.
#[page(POST "./receive")]
pub async fn receive_parcel(
    cx: &Cx,
    Form(form): Form<HashMap<String, String>>,
) -> Result<impl View> {
    let (id, request) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let mut lines = Vec::with_capacity(request.lines.len());
    for index in 0..request.lines.len() {
        let Some(product_id) = form.get(&format!("product_{index}")) else {
            continue;
        };
        lines.push(ReceivedLine {
            product_id: product_id.clone(),
            accepted: form
                .get(&format!("accepted_{index}"))
                .and_then(|a| a.trim().parse().ok())
                .unwrap_or(0),
            restock: form.contains_key(&format!("restock_{index}")),
        });
    }
    let refund_method = match form.get("refund_method").map(String::as_str) {
        Some("credit") => RefundMethod::StoreCredit,
        _ => RefundMethod::OriginalPayment,
    };
    let outcome = returns(services)
        .receive_return(
            &id,
            ReceiveReturn {
                lines,
                refund_method,
            },
        )
        .await;
    Err::<(), _>(see_other(settled(cx, &id, outcome)?).into())
}
