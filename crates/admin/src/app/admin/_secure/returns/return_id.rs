//! `/{mount}/returns/{return_id}`: one return — review the request, then
//! record the parcel: what is taken back, what goes into stock again, and how
//! the customer is refunded — or that the same products are sent again
//! instead. The rest is the returns process manager's job, up to the
//! replacement parcel, which is handed to the carrier from here.

use std::collections::HashMap;

use serde::Deserialize;
use timada_order::order_numbers_by_ids;
use timada_returns::{
    IssueLabel, LabelFile, ReceiveReturn, ReceivedLine, RefundMethod, ReplacementStatus,
    ReturnError, ReturnGround, ReturnStatus, ReturnView, load_return, load_return_label_file,
};
use timada_shipping::ShipmentStatus;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::{Form, multipart::Multipart},
        error::RouterErrorExt,
        error::see_other,
        href, page, path_param, path_param as param, query_params, query_params as query,
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
    config::{AdminConfig, AdminServices},
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
        "stock" => Some(
            "Le produit n'est plus en stock à l'entrepôt : il ne peut pas être remplacé. Remboursez ce retour.",
        ),
        "nothing" => Some("Aucun article repris : il n'y a rien à remplacer."),
        "parcel" => Some("Le colis de remplacement n'attend plus d'être expédié."),
        "label" => Some("Une étiquette, c'est un lien, un fichier, ou les deux."),
        "label-url" => Some("Le lien de l'étiquette doit commencer par https:// ou http://."),
        "label-file" => {
            Some("Le fichier de l'étiquette doit être un PDF, un PNG ou un JPEG de 5 Mo au plus.")
        }
        "label-exists" => Some("Ce retour a déjà son étiquette."),
        "label-field" => Some("Indiquez le transporteur et le numéro de suivi de l'étiquette."),
        "carrier" => Some(
            "Le transporteur n'a pas pu fournir l'étiquette. Réessayez, ou joignez-la à la main.",
        ),
        _ => None,
    }
}

fn returns(cx: &Cx) -> timada_returns::Command<'_, evento::Evento> {
    let services = app_context::<AdminServices>(cx);
    timada_returns::Command {
        executor: &services.executor,
        db: services.db.clone(),
        policy: app_context::<AdminConfig>(cx).return_policy.clone(),
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
        Err(ReturnError::Required("carrier" | "tracking_number")) => "label-field",
        Err(ReturnError::Required(_)) => "reason",
        Err(ReturnError::ReplacementOutOfStock(_)) => "stock",
        Err(ReturnError::NothingToReplace) => "nothing",
        Err(ReturnError::LabelMissing) => "label",
        Err(ReturnError::InvalidLabelUrl) => "label-url",
        Err(ReturnError::InvalidLabelFile) => "label-file",
        Err(ReturnError::LabelAlreadyIssued) => "label-exists",
        Err(ReturnError::LabelProvider(error)) => {
            tracing::warn!(return_id = %id, %error, "return label provider failed");
            "carrier"
        }
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

/// The prepaid label of a return, worded.
struct IssuedLabel {
    tracking: String,
    link: Option<String>,
    /// `(where, file name)`.
    download: Option<(String, String)>,
    cost: String,
}

/// The replacement of a return, worded.
struct Replacement {
    what: Vec<String>,
    state: String,
    tracking: Option<String>,
    can_dispatch: bool,
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
    // The prepaid label: what the customer was given, and what it costs them.
    let ground = request.ground.map(ReturnGround::label);
    let label = request.label.as_ref().map(|issued| IssuedLabel {
        tracking: format!("{} — {}", issued.carrier, issued.tracking_number),
        link: issued.url.clone(),
        download: issued.file_name.as_ref().map(|name| {
            (
                href!(label_file, ReturnId(id.clone())).resolve(cx),
                name.clone(),
            )
        }),
        cost: if issued.fee.is_positive() {
            format!(
                "{} à la charge du client, déduits du remboursement",
                money(&issued.fee)
            )
        } else {
            "Offerte au client".to_owned()
        },
    });
    // What a label would cost this customer, said before it is given.
    let currency = request.money.currency.clone();
    let policy_fee =
        app_context::<AdminConfig>(cx)
            .return_policy
            .label_fee(request.ground, false, &currency);
    let fee_notice = if policy_fee.is_positive() {
        format!(
            "Selon la politique de retour, {} seront déduits du remboursement.",
            money(&policy_fee)
        )
    } else {
        "Selon la politique de retour, l'étiquette est offerte (boutique en cause, ou étiquettes gratuites).".to_owned()
    };
    let can_label = request.status == ReturnStatus::Approved && request.label.is_none();
    let has_carrier = services.return_labels.is_some();
    let fee_deducted = request.label_fee_deducted.as_ref().map(money);

    // The replacement, and its parcel once there is one: `(what, state,
    // tracking, can be dispatched)`.
    let replacement = match &request.replacement {
        None => None,
        Some(replacement) => {
            let parcel = match &replacement.shipment_id {
                Some(shipment_id) => {
                    timada_shipping::load_shipment(&services.executor, shipment_id).await?
                }
                None => None,
            };
            let state = match (replacement.status, parcel.as_ref().map(|p| p.status)) {
                (ReplacementStatus::Planned, _) => "Décidé : le stock va être réservé.".to_owned(),
                (ReplacementStatus::Abandoned, _) => format!(
                    "Impossible ({}) : le retour a été remboursé.",
                    replacement
                        .abandoned_reason
                        .clone()
                        .unwrap_or_else(|| "motif inconnu".to_owned())
                ),
                (ReplacementStatus::Arranged, Some(ShipmentStatus::Created)) => {
                    "Colis prêt : à remettre au transporteur.".to_owned()
                }
                (ReplacementStatus::Arranged, Some(ShipmentStatus::Cancelled)) => {
                    "Colis annulé.".to_owned()
                }
                (ReplacementStatus::Arranged, _) => "Colis expédié.".to_owned(),
            };
            Some(Replacement {
                what: replacement
                    .lines
                    .iter()
                    .map(|line| format!("{} × {}", line.quantity, line.name))
                    .collect(),
                state,
                tracking: parcel.as_ref().and_then(|p| {
                    p.carrier
                        .clone()
                        .zip(p.tracking_number.clone())
                        .map(|(carrier, number)| format!("{carrier} — {number}"))
                }),
                can_dispatch: parcel.is_some_and(|p| p.status == ShipmentStatus::Created),
            })
        }
    };

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
                                <fieldset class="flex flex-col gap-1">
                                    <legend class="text-muted-foreground">"Suite donnée"</legend>
                                    <label class="flex items-center gap-2"><input type="radio" name="settlement" value="refund" checked=(true)> "Rembourser"</label>
                                    <label class="flex items-center gap-2"><input type="radio" name="settlement" value="replace"> "Remplacer par le même produit (défectueux, abîmé, erreur d'envoi)"</label>
                                </fieldset>
                                <div class="flex flex-col gap-1">
                                    <label for="refund_method" class="text-muted-foreground">"Remboursement — ou à défaut de stock pour le remplacement"</label>
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
                if let Some(replacement) = &replacement {
                    card(
                        card_header(card_title("Remplacement"))
                        card_content(
                            <div class="flex flex-col gap-2 text-sm">
                                <ul>
                                    for item in &replacement.what { <li>(item.clone())</li> }
                                </ul>
                                <p role="status">(replacement.state.clone())</p>
                                if let Some(tracking) = &replacement.tracking {
                                    <p class="font-mono text-xs">(tracking.clone())</p>
                                }
                                if replacement.can_dispatch {
                                    <form method="post" action=(href!(dispatch_replacement, ReturnId(id.clone()))) class="mt-2 flex flex-col gap-2">
                                        input(attrs: topcoat::view::attributes! { name="carrier" placeholder="Transporteur" aria-label="Transporteur" required=(true) value="Colissimo" })
                                        input(attrs: topcoat::view::attributes! { name="tracking_number" placeholder="N° de suivi" aria-label="N° de suivi" required=(true) })
                                        button(attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Expédier le remplacement")
                                    </form>
                                }
                            </div>
                        )
                    )
                }
                if let Some(label) = &label {
                    card(
                        card_header(card_title("Étiquette de retour"))
                        card_content(
                            <div class="flex flex-col gap-2 text-sm">
                                <p class="font-mono text-xs">(label.tracking.clone())</p>
                                <p>(label.cost.clone())</p>
                                if let Some((link, name)) = &label.download {
                                    <a href=(link.clone()) class="underline underline-offset-4">"Télécharger " (name.clone())</a>
                                }
                                if let Some(link) = &label.link {
                                    <a href=(link.clone()) rel="noopener noreferrer" class="break-all underline underline-offset-4">(link.clone())</a>
                                }
                            </div>
                        )
                    )
                }
                if can_label {
                    card(
                        card_header(card_title("Étiquette de retour"))
                        card_content(
                            <div class="flex flex-col gap-3 text-sm">
                                <p class="text-muted-foreground">(fee_notice.clone())</p>
                                if has_carrier {
                                    <form method="post" action=(href!(provide_label, ReturnId(id.clone()))) class="flex flex-col gap-2">
                                        <label class="flex items-center gap-2"><input type="checkbox" name="waive_fee" value="on"> "Offrir l'étiquette au client"</label>
                                        button(attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Demander l'étiquette au transporteur")
                                    </form>
                                    separator()
                                }
                                <form method="post" enctype="multipart/form-data" action=(href!(attach_label, ReturnId(id.clone()))) class="flex flex-col gap-2">
                                    label_fields()
                                    button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Joindre l'étiquette")
                                </form>
                            </div>
                        )
                    )
                }
                card(
                    card_header(card_title("Demande"))
                    card_content(
                        <dl class="flex flex-col gap-2 text-sm">
                            <div><dt class="text-muted-foreground">"Commande"</dt><dd><a href=(order_link) class="font-mono text-xs underline-offset-4 hover:underline">(order_label)</a></dd></div>
                            <div><dt class="text-muted-foreground">"Client"</dt><dd><a href=(customer_link) class="font-mono text-xs underline-offset-4 hover:underline">(request.customer_id.clone())</a></dd></div>
                            if let Some(ground) = ground {
                                <div><dt class="text-muted-foreground">"Nature du retour"</dt><dd>(ground)</dd></div>
                            }
                            <div><dt class="text-muted-foreground">"Motif"</dt><dd>(request.reason.clone())</dd></div>
                            if let Some(reason) = &request.refused_reason {
                                <div><dt class="text-muted-foreground">"Motif du refus"</dt><dd>(reason.clone())</dd></div>
                            }
                            if let Some(method) = refund_method {
                                <div><dt class="text-muted-foreground">"Remboursement"</dt><dd>(method)</dd></div>
                                <div><dt class="text-muted-foreground">"Sur le paiement"</dt><dd class="tabular-nums">(money(&request.money))</dd></div>
                                <div><dt class="text-muted-foreground">"En avoir"</dt><dd class="tabular-nums">(money(&request.credit))</dd></div>
                            }
                            if let Some(fee) = &fee_deducted {
                                <div><dt class="text-muted-foreground">"Étiquette déduite"</dt><dd class="tabular-nums">(fee.clone())</dd></div>
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
                                <form method="post" enctype="multipart/form-data" action=(href!(approve, ReturnId(id.clone()))) class="flex flex-col gap-2 text-sm">
                                    <details>
                                        <summary class="cursor-pointer text-muted-foreground">"Joindre une étiquette de retour prépayée"</summary>
                                        <div class="mt-2 flex flex-col gap-2">
                                            <p class="text-muted-foreground">(fee_notice.clone())</p>
                                            label_fields()
                                        </div>
                                    </details>
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

/// The fields of a label, shared by the approval and the later attachment.
#[topcoat::view::component]
async fn label_fields() -> Result<impl View> {
    Ok(view! {
        input(attrs: topcoat::view::attributes! { name="label_carrier" placeholder="Transporteur" aria-label="Transporteur de l'étiquette" value="Colissimo" })
        input(attrs: topcoat::view::attributes! { name="label_tracking" placeholder="N° de suivi du retour" aria-label="Numéro de suivi de l'étiquette" })
        input(attrs: topcoat::view::attributes! { name="label_url" type="url" placeholder="Lien vers l'étiquette (https://…)" aria-label="Lien vers l'étiquette" })
        <label class="flex flex-col gap-1 text-muted-foreground">
            "Ou le fichier de l'étiquette (PDF, PNG, JPEG — 5 Mo)"
            <input type="file" name="label_file" accept="application/pdf,image/png,image/jpeg">
        </label>
        <label class="flex items-center gap-2"><input type="checkbox" name="waive_fee" value="on"> "Offrir l'étiquette au client"</label>
    })
}

/// The label an operator filled in, if they did: a tracking number, a link
/// or a file says they meant to.
async fn posted_label(multipart: Option<Multipart>) -> Result<Option<IssueLabel>> {
    let Some(mut multipart) = multipart else {
        return Ok(None);
    };
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file = None;
    while let Some(field) = multipart.next_field().await? {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "label_file" {
            let file_name = field.file_name().unwrap_or_default().to_owned();
            let content_type = field.content_type().unwrap_or_default().to_owned();
            let bytes = field.bytes().await?;
            // A file input left empty still posts a nameless, empty part.
            if !bytes.is_empty() {
                file = Some(LabelFile {
                    file_name,
                    content_type,
                    bytes: bytes.to_vec(),
                });
            }
        } else {
            fields.insert(name, field.text().await?);
        }
    }
    let text = |name: &str| fields.get(name).map_or("", |v| v.trim()).to_owned();
    let (tracking_number, url) = (text("label_tracking"), text("label_url"));
    if tracking_number.is_empty() && url.is_empty() && file.is_none() {
        return Ok(None);
    }
    Ok(Some(IssueLabel {
        carrier: text("label_carrier"),
        tracking_number,
        url: Some(url).filter(|url| !url.is_empty()),
        file,
        waive_fee: fields.contains_key("waive_fee"),
    }))
}

/// Accepts the return — with its prepaid label when the operator joined one,
/// so the e-mail announcing the approval carries it.
#[page(POST "./approve")]
pub async fn approve(cx: &Cx, multipart: Option<Multipart>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let outcome = match posted_label(multipart).await? {
        Some(label) => returns(cx).approve_return_with_label(&id, label).await,
        None => returns(cx).approve_return(&id).await,
    };
    Err::<(), _>(see_other(settled(cx, &id, outcome)?).into())
}

/// Joins the label to a return already approved.
#[page(POST "./label")]
pub async fn attach_label(cx: &Cx, multipart: Option<Multipart>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let outcome = match posted_label(multipart).await? {
        Some(label) => returns(cx).issue_return_label(&id, label).await,
        None => Err(ReturnError::LabelMissing),
    };
    Err::<(), _>(see_other(settled(cx, &id, outcome)?).into())
}

#[derive(Debug, Deserialize)]
pub struct ProvideLabelForm {
    waive_fee: Option<String>,
}

/// Asks the carrier plugged into the admin for the label.
#[page(POST "./label/provide")]
pub async fn provide_label(cx: &Cx, Form(form): Form<ProvideLabelForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let outcome = match &services.return_labels {
        Some(labels) => {
            returns(cx)
                .provide_return_label(&id, labels.0.as_ref(), form.waive_fee.is_some())
                .await
        }
        None => Err(ReturnError::LabelMissing),
    };
    Err::<(), _>(see_other(settled(cx, &id, outcome)?).into())
}

/// A label's file, handed to the browser under its own type.
pub struct LabelDownload(LabelFile);

impl topcoat::router::response::IntoResponse for LabelDownload {
    fn into_response(self, _cx: &Cx) -> Result<topcoat::router::response::Response> {
        Ok(topcoat::router::response::Response::builder()
            .header("Content-Type", self.0.content_type)
            .header(
                "Content-Disposition",
                format!("attachment; filename=\"{}\"", self.0.file_name),
            )
            .header("X-Content-Type-Options", "nosniff")
            .header("Cache-Control", "private, no-store")
            .body(topcoat::router::Body::from(self.0.bytes))?)
    }
}

/// `./label`: the label's file, as the customer gets it.
#[topcoat::router::route(GET "./label")]
pub async fn label_file(cx: &Cx) -> Result<LabelDownload> {
    let (id, _) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let file = load_return_label_file(&services.db, &id)
        .await?
        .ok_or_not_found()?;
    Ok(LabelDownload(file))
}

#[derive(Debug, Deserialize)]
pub struct RefuseForm {
    reason: String,
}

#[page(POST "./refuse")]
pub async fn refuse(cx: &Cx, Form(form): Form<RefuseForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    let outcome = returns(cx).refuse_return(&id, form.reason).await;
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
    let outcome = returns(cx)
        .receive_return(
            &id,
            ReceiveReturn {
                lines,
                refund_method,
                replace: form
                    .get("settlement")
                    .is_some_and(|chosen| chosen == "replace"),
            },
        )
        .await;
    Err::<(), _>(see_other(settled(cx, &id, outcome)?).into())
}

#[derive(Debug, Deserialize)]
pub struct DispatchForm {
    carrier: String,
    tracking_number: String,
}

/// Hands the replacement parcel to the carrier.
#[page(POST "./replacement/dispatch")]
pub async fn dispatch_replacement(cx: &Cx, Form(form): Form<DispatchForm>) -> Result<impl View> {
    let (id, request) = load(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let shipment_id = request.replacement.and_then(|r| r.shipment_id);
    let dispatched = match shipment_id {
        Some(shipment_id) => timada_shipping::Command(&services.executor)
            .dispatch_shipment(
                shipment_id,
                form.carrier.trim().to_owned(),
                form.tracking_number.trim().to_owned(),
            )
            .await
            .map_err(Some),
        None => Err(None),
    };
    let target = match dispatched {
        Ok(()) => back(cx, &id),
        Err(None | Some(timada_shipping::ShippingError::NotCreated)) => {
            format!("{}?error=parcel", back(cx, &id))
        }
        Err(Some(err)) => return Err(anyhow::Error::from(err).into()),
    };
    Err::<(), _>(see_other(target).into())
}
