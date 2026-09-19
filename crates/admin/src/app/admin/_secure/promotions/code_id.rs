//! `/{mount}/promotions/{code_id}`: one promo code or voucher, its usage and
//! the action that ends it. `code_id` is the aggregate id, whichever the kind.

use serde::Deserialize;
use timada_promotion::{
    DiscountKind, DiscountView, VoucherKind, VoucherView, load_discount_details,
    load_voucher_balance,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param,
    },
    view::{View, view},
};

use super::code_value;
use crate::{
    app::admin::_secure::orders::order_id,
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
    },
    config::AdminServices,
    ui::{date, money, page_header},
};

path_param!(pub code_id: String, error = not_found);

enum Code {
    Discount(DiscountView),
    Voucher(VoucherView),
}

async fn load(cx: &Cx) -> Result<(String, Code)> {
    let id = param::<CodeId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    if let Some(discount) = load_discount_details(&services.executor, &id).await? {
        return Ok((id, Code::Discount(discount)));
    }
    let voucher = load_voucher_balance(&services.executor, &id)
        .await?
        .ok_or_not_found()?;
    Ok((id, Code::Voucher(voucher)))
}

fn back(cx: &Cx, id: &str) -> String {
    href!(show, CodeId(id.to_owned())).resolve(cx)
}

fn promotion(services: &AdminServices) -> timada_promotion::Command<'_, evento::Evento> {
    timada_promotion::Command {
        executor: &services.executor,
        db: services.db.clone(),
    }
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let (id, code) = load(cx).await?;
    Ok(view! {
        match &code {
            Code::Discount(discount) => discount_detail(id: &id, discount: discount),
            Code::Voucher(voucher) => voucher_detail(id: &id, voucher: voucher),
        }
    })
}

#[topcoat::view::component]
async fn discount_detail(id: &str, discount: &DiscountView) -> Result<impl View> {
    let value = match &discount.kind {
        DiscountKind::Percent { bp } => code_value(Some(i64::from(*bp)), None),
        DiscountKind::FixedAmount { amount } => code_value(None, Some(amount.clone())),
    };
    let uses = match discount.max_redemptions {
        Some(max) => format!("{} / {max}", discount.redeemed),
        None => format!("{} (illimité)", discount.redeemed),
    };
    let valid_until = discount
        .valid_until
        .map_or_else(|| "Sans limite".to_owned(), date);
    Ok(view! {
        page_header(
            title: &discount.code,
            <span class="text-sm text-muted-foreground">(if discount.active { "Code promo · actif" } else { "Code promo · inactif" })</span>
        )
        <div class="grid gap-6 lg:grid-cols-3">
            <div class="lg:col-span-2">
                card(
                    card_header(card_title("Règle"))
                    card_content(
                        <dl class="grid grid-cols-2 gap-y-2 text-sm">
                            <dt class="text-muted-foreground">"Remise sur les articles"</dt><dd class="text-right tabular-nums">(value)</dd>
                            <dt class="text-muted-foreground">"Utilisations"</dt><dd class="text-right tabular-nums">(uses)</dd>
                            <dt class="text-muted-foreground">"Valable jusqu'au"</dt><dd class="text-right">(valid_until)</dd>
                        </dl>
                    )
                )
            </div>
            if discount.active {
                card(
                    card_header(card_title("Actions"))
                    card_content(
                        <form method="post" action=(href!(deactivate, CodeId(id.to_owned())))>
                            button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Désactiver le code")
                        </form>
                    )
                )
            }
        </div>
    })
}

#[topcoat::view::component]
async fn voucher_detail(cx: &Cx, id: &str, voucher: &VoucherView) -> Result<impl View> {
    let kind = match &voucher.kind {
        VoucherKind::GiftVoucher => "Bon d'achat".to_owned(),
        VoucherKind::CreditNote { origin_order_id } => {
            format!("Avoir (commande {origin_order_id})")
        }
    };
    let state = if voucher.cancelled {
        "annulé"
    } else {
        "actif"
    };
    let expires_at = voucher
        .expires_at
        .map_or_else(|| "Sans limite".to_owned(), date);
    let holder = voucher
        .customer_id
        .clone()
        .unwrap_or_else(|| "Au porteur".to_owned());
    let db = &app_context::<AdminServices>(cx).db;
    let order_ids: Vec<String> = voucher
        .redemptions
        .iter()
        .map(|r| r.order_id.clone())
        .collect();
    let order_numbers = timada_order::order_numbers_by_ids(db, &order_ids).await?;
    let redemptions: Vec<(String, String, String)> = voucher
        .redemptions
        .iter()
        .map(|r| {
            (
                href!(order_id::show, order_id::OrderId(r.order_id.clone())).resolve(cx),
                order_numbers
                    .get(&r.order_id)
                    .unwrap_or(&r.order_id)
                    .clone(),
                money(&r.amount),
            )
        })
        .collect();
    Ok(view! {
        page_header(
            title: &voucher.code,
            <span class="text-sm text-muted-foreground">(kind) " · " (state)</span>
        )
        <div class="grid gap-6 lg:grid-cols-3">
            <div class="flex flex-col gap-6 lg:col-span-2">
                card(
                    card_header(card_title("Solde"))
                    card_content(
                        <dl class="grid grid-cols-2 gap-y-2 text-sm">
                            <dt class="text-muted-foreground">"Valeur initiale"</dt><dd class="text-right tabular-nums">(money(&voucher.value))</dd>
                            <dt class="text-muted-foreground">"Restant"</dt><dd class="text-right font-semibold tabular-nums">(money(&voucher.remaining))</dd>
                            <dt class="text-muted-foreground">"Titulaire"</dt><dd class="text-right">(holder)</dd>
                            <dt class="text-muted-foreground">"Expire le"</dt><dd class="text-right">(expires_at)</dd>
                            if let Some(reason) = &voucher.cancelled_reason {
                                <dt class="text-muted-foreground">"Motif d'annulation"</dt><dd class="text-right">(reason.clone())</dd>
                            }
                        </dl>
                    )
                )
                card(
                    card_header(card_title("Utilisations"))
                    card_content(
                        if redemptions.is_empty() {
                            <p class="text-sm text-muted-foreground">"Pas encore utilisé."</p>
                        } else {
                            <table class="w-full text-sm">
                                <tbody>
                                    for (link, order, amount) in &redemptions {
                                        <tr class="border-b border-border last:border-0">
                                            <td class="py-2"><a href=(link.clone()) class="font-mono text-xs underline-offset-4 hover:underline">(order.clone())</a></td>
                                            <td class="py-2 text-right tabular-nums">(amount.clone())</td>
                                        </tr>
                                    }
                                </tbody>
                            </table>
                        }
                    )
                )
            </div>
            if !voucher.cancelled {
                card(
                    card_header(card_title("Actions"))
                    card_content(
                        <form method="post" action=(href!(cancel, CodeId(id.to_owned()))) class="flex flex-col gap-2">
                            input(attrs: topcoat::view::attributes! { name="reason" placeholder="Motif" required=(true) aria-label="Motif d'annulation" })
                            button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Annuler le solde restant")
                        </form>
                    )
                )
            }
        </div>
    })
}

#[page(POST "./deactivate")]
pub async fn deactivate(cx: &Cx) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    promotion(app_context::<AdminServices>(cx))
        .deactivate_discount(&id)
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}

#[derive(Debug, Deserialize)]
pub struct CancelForm {
    reason: String,
}

#[page(POST "./cancel")]
pub async fn cancel(cx: &Cx, Form(form): Form<CancelForm>) -> Result<impl View> {
    let (id, _) = load(cx).await?;
    promotion(app_context::<AdminServices>(cx))
        .cancel_voucher(&id, form.reason)
        .await?;
    Err::<(), _>(see_other(back(cx, &id)).into())
}
