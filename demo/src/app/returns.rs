//! Returns ("retours"): asking to send lines of a shipped order back, and the
//! return slip that follows a request from review to refund.

use std::collections::HashMap;

use timada_order::{OrderDetailsView, OrderStatus, load_order_details};
use timada_returns::{
    RequestReturn, RequestedLine, ReturnError, ReturnPolicy, ReturnStatus, claimed_quantities,
    load_return,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param,
    },
    view::{View, component, view},
};

use super::{
    account,
    checkout::OrderId,
    document,
    format::{date, money},
};
use crate::{Store, auth::require_account, db::RETURNS_ADDRESS};

path_param!(pub return_id: String, error = not_found);

/// The reasons offered in the form; free text goes in the details.
const REASONS: [&str; 4] = [
    "Ne convient pas",
    "Produit défectueux",
    "Erreur de commande",
    "Autre",
];

fn returns(store: &Store) -> timada_returns::Command<'_, evento::Sqlite> {
    timada_returns::Command {
        executor: &store.executor,
        db: store.db.clone(),
        policy: ReturnPolicy::default(),
    }
}

pub fn return_status(status: &str) -> &'static str {
    match status {
        "requested" => "Demande en cours d'examen",
        "approved" => "Accepté — en attente de votre colis",
        "refused" => "Refusé",
        "cancelled" => "Annulé",
        "received" => "Colis reçu — remboursement en cours",
        "completed" => "Traité",
        _ => "Inconnu",
    }
}

/// One line of the order with what can still be sent back.
struct ReturnableLine {
    product_id: String,
    name: String,
    unit_price: String,
    returnable: u32,
}

/// Until when the order can be returned, if it still can.
pub fn return_deadline(order: &OrderDetailsView) -> anyhow::Result<Option<u64>> {
    let Some(shipped_at) = order
        .shipped_at
        .filter(|_| order.status == OrderStatus::Shipped)
    else {
        return Ok(None);
    };
    let deadline = ReturnPolicy::default().deadline(shipped_at);
    Ok((timada_core::time::now_unix_secs()? <= deadline).then_some(deadline))
}

/// What is left to return on each line: bought minus what the order's
/// returns already hold.
async fn returnable_lines(
    store: &Store,
    order: &OrderDetailsView,
) -> anyhow::Result<Vec<ReturnableLine>> {
    let claimed: HashMap<String, i64> = claimed_quantities(&store.db, &order.id)
        .await?
        .into_iter()
        .collect();
    Ok(order
        .lines
        .iter()
        .map(|line| {
            let held = claimed.get(&line.product_id).copied().unwrap_or(0);
            ReturnableLine {
                product_id: line.product_id.clone(),
                name: line.name.clone(),
                unit_price: money(&line.unit_price),
                returnable: (i64::from(line.quantity) - held).max(0) as u32,
            }
        })
        .collect())
}

/// Whether the order page should offer a return: inside the window, with
/// something left to send back.
pub async fn can_request_return(store: &Store, order: &OrderDetailsView) -> anyhow::Result<bool> {
    if return_deadline(order)?.is_none() {
        return Ok(false);
    }
    Ok(returnable_lines(store, order)
        .await?
        .iter()
        .any(|l| l.returnable > 0))
}

async fn own_order(cx: &Cx) -> Result<OrderDetailsView> {
    let account = require_account(cx).await?;
    let id = param::<OrderId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    Ok(load_order_details(&store.executor, &id)
        .await?
        .filter(|o| o.customer_id == account.customer_id)
        .ok_or_not_found()?)
}

#[page("/account/orders/{order_id}/return")]
pub async fn new_return(cx: &Cx) -> Result<impl View> {
    own_order(cx).await?;
    Ok(view! { return_form(error: None) })
}

/// Files the request. What the returns context refuses comes back as a
/// message on the form.
#[page(POST "/account/orders/{order_id}/return")]
pub async fn request_return(
    cx: &Cx,
    Form(form): Form<HashMap<String, String>>,
) -> Result<impl View> {
    let order = own_order(cx).await?;
    let store = app_context::<Store>(cx);

    // `product_<i>` / `quantity_<i>` pairs, one per line of the form.
    let mut lines = Vec::new();
    for index in 0..order.lines.len() {
        let product_id = form.get(&format!("product_{index}"));
        let quantity = form
            .get(&format!("quantity_{index}"))
            .and_then(|q| q.trim().parse::<u32>().ok())
            .unwrap_or(0);
        if let Some(product_id) = product_id.filter(|_| quantity > 0) {
            lines.push(RequestedLine {
                product_id: product_id.clone(),
                quantity,
            });
        }
    }
    let reason = form.get("reason").map_or("", |r| r.trim());
    let details = form.get("details").map_or("", |d| d.trim());
    let reason = match (reason, details) {
        ("", details) => details.to_owned(),
        (reason, "") => reason.to_owned(),
        (reason, details) => format!("{reason} — {details}"),
    };

    let requested = returns(store)
        .request_return(RequestReturn {
            order_id: order.id.clone(),
            customer_id: order.customer_id.clone(),
            lines,
            reason,
        })
        .await;
    let error = match requested {
        Ok(id) => return Err(see_other(href!(show, ReturnId(id)).resolve(cx)).into()),
        Err(ReturnError::NoLines) => "Indiquez au moins un article à retourner.".to_owned(),
        Err(ReturnError::Required(_)) => "Indiquez le motif du retour.".to_owned(),
        Err(ReturnError::QuantityExceeded { returnable, .. }) => {
            format!("Vous ne pouvez plus retourner que {returnable} exemplaire(s) de cet article.")
        }
        Err(ReturnError::WindowClosed) => {
            "Le délai de retour de cette commande est dépassé.".to_owned()
        }
        Err(ReturnError::OrderNotShipped) => {
            "Seule une commande expédiée peut être retournée.".to_owned()
        }
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Ok(view! { return_form(error: Some(error)) })
}

#[component]
async fn return_form(cx: &Cx, error: Option<String>) -> Result<impl View> {
    let order = own_order(cx).await?;
    let store = app_context::<Store>(cx);
    let deadline = return_deadline(&order)?.map(date);
    let lines = returnable_lines(store, &order).await?;
    let anything_left = lines.iter().any(|l| l.returnable > 0);
    // Owned: a view cannot borrow from a local.
    let numbered: Vec<(usize, ReturnableLine)> = lines.into_iter().enumerate().collect();
    let action = href!(request_return, OrderId(order.id.clone())).resolve(cx);
    let back = href!(account::order_detail, OrderId(order.id.clone())).resolve(cx);

    Ok(view! {
        document(
            title: "Retourner des articles",
            <h1>"Retourner des articles"</h1>
            <p>"Commande " (order.display_number().to_owned())</p>
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            match &deadline {
                Some(until) if anything_left => {
                    <p class="muted">"Vous pouvez demander un retour jusqu'au " (until.clone()) ". Les frais de port ne sont pas remboursés."</p>
                    <form method="post" action=(action.clone())>
                        <table>
                            <caption class="muted">"Articles à retourner"</caption>
                            <thead>
                                <tr>
                                    <th scope="col">"Produit"</th>
                                    <th scope="col" class="num">"Prix payé"</th>
                                    <th scope="col" class="num">"Retournable"</th>
                                    <th scope="col">"Quantité à retourner"</th>
                                </tr>
                            </thead>
                            <tbody>
                                for (index, line) in &numbered {
                                    <tr>
                                        <th scope="row">(line.name.clone())</th>
                                        <td class="num">(line.unit_price.clone())</td>
                                        <td class="num">(line.returnable.to_string())</td>
                                        <td>
                                            <input type="hidden" name=(format!("product_{index}")) value=(line.product_id.clone())>
                                            <input type="number" name=(format!("quantity_{index}")) min="0" max=(line.returnable.to_string()) value="0" aria-label=(format!("Quantité de {} à retourner", line.name)) disabled=(line.returnable == 0)>
                                        </td>
                                    </tr>
                                }
                            </tbody>
                        </table>
                        <p>
                            <label for="reason">"Motif du retour"</label>
                            <select id="reason" name="reason" required=(true)>
                                for reason in REASONS { <option value=(reason)>(reason)</option> }
                            </select>
                        </p>
                        <p>
                            <label for="details">"Précisions (facultatif)"</label>
                            <textarea id="details" name="details" rows="3" cols="60" maxlength="1000"></textarea>
                        </p>
                        <button type="submit">"Demander le retour"</button>
                    </form>
                }
                Some(_) => {
                    <p class="muted">"Tous les articles de cette commande font déjà l'objet d'un retour."</p>
                }
                None => {
                    <p class="muted">"Cette commande ne peut pas, ou plus, être retournée."</p>
                }
            }
            <p><a href=(back)>"Retour à la commande"</a></p>
        )
    })
}

/// The return slip: what is coming back, where to send it, and where the
/// request stands. Someone else's return is a 404.
#[page("/account/returns/{return_id}")]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<ReturnId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let request = load_return(&store.executor, &id)
        .await?
        .filter(|r| r.customer_id == account.customer_id)
        .ok_or_not_found()?;

    let order_link = href!(account::order_detail, OrderId(request.order_id.clone())).resolve(cx);
    let cancel_action = href!(cancel, ReturnId(id.clone())).resolve(cx);
    let lines: Vec<(String, String, String)> = request
        .lines
        .iter()
        .map(|line| {
            let accepted = request
                .received
                .iter()
                .find(|r| r.product_id == line.product_id)
                .map_or_else(|| "—".to_owned(), |r| r.accepted.to_string());
            (line.name.clone(), line.quantity.to_string(), accepted)
        })
        .collect();
    let address: Vec<&str> = RETURNS_ADDRESS.lines().collect();
    let refunded = request.money.is_positive().then(|| money(&request.money));
    let credited = request
        .voucher_code
        .clone()
        .filter(|_| request.credit.is_positive())
        .map(|code| (money(&request.credit), code));

    Ok(view! {
        document(
            title: "Bon de retour",
            <h1>"Retour " (request.rma_number.clone())</h1>
            <p>
                "Demandé le " (date(request.requested_at)) " · "
                <strong>(return_status(request.status.as_str()))</strong>
            </p>
            <p>"Motif : " (request.reason.clone())</p>
            if let Some(reason) = &request.refused_reason {
                <p role="status" class="notice">"Demande refusée : " (reason.clone())</p>
            }
            <table>
                <caption class="muted">"Articles du retour"</caption>
                <thead>
                    <tr>
                        <th scope="col">"Produit"</th>
                        <th scope="col" class="num">"Demandé"</th>
                        <th scope="col" class="num">"Repris"</th>
                    </tr>
                </thead>
                <tbody>
                    for (name, quantity, accepted) in &lines {
                        <tr>
                            <th scope="row">(name.clone())</th>
                            <td class="num">(quantity.clone())</td>
                            <td class="num">(accepted.clone())</td>
                        </tr>
                    }
                </tbody>
            </table>
            if request.status == ReturnStatus::Approved {
                <div class="card">
                    <h2>"Envoyer votre colis"</h2>
                    <p>"Inscrivez le numéro " <strong>(request.rma_number.clone())</strong> " sur le colis et envoyez-le à :"</p>
                    <address>for line in &address { (*line) <br> }</address>
                </div>
            }
            if let Some(refunded) = &refunded {
                <p role="status" class="notice">"Remboursement sur votre moyen de paiement : " <strong>(refunded.clone())</strong> " — un e-mail vous confirme son arrivée."</p>
            }
            if let Some((amount, code)) = &credited {
                <p role="status" class="notice">"Avoir de " <strong>(amount.clone())</strong> " : saisissez le code " <strong>(code.clone())</strong> " dans votre panier."</p>
            }
            if request.status.is_open() {
                <form method="post" action=(cancel_action)>
                    <button type="submit" class="link">"Annuler cette demande de retour"</button>
                </form>
            }
            <p><a href=(order_link)>"Retour à la commande"</a></p>
        )
    })
}

#[page(POST "/account/returns/{return_id}/cancel")]
pub async fn cancel(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<ReturnId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    match returns(store)
        .cancel_return(&id, &account.customer_id)
        .await
    {
        // Already received in the meantime: the slip says where it stands.
        Ok(()) | Err(ReturnError::WrongStatus { .. }) => {}
        Err(ReturnError::ReturnNotFound) => None::<()>.ok_or_not_found()?,
        Err(err) => return Err(anyhow::Error::from(err).into()),
    }
    Err::<(), _>(see_other(href!(show, ReturnId(id)).resolve(cx)).into())
}
