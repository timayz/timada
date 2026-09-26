//! `/{mount}/customers/{customer_id}`: profile, company identity, address book
//! and order history.

use timada_core::{Address, Money};
use timada_customer::{load_address_book, load_company_identity};
use timada_order::orders_of_customer;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::RouterErrorExt, href, page, path_param, path_param as param},
    view::{View, view},
};

use crate::{
    app::admin::_secure::orders::order_id::{OrderId, show as order_show},
    auth::Section,
    components::card::{card, card_content, card_header, card_title},
    config::AdminServices,
    ui::{date, detail_grid, detail_main, detail_side, fact, facts, link, money, page_header},
};

path_param!(pub customer_id: String, error = not_found);

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let id = param::<CustomerId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let book = load_address_book(&services.executor, &id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_not_found()?;
    let orders = orders_of_customer(&services.db, &id).await?;
    let title = format!("{} {}", book.first_name, book.last_name);
    let standing = if book.guest {
        "Invité (sans compte) · "
    } else {
        ""
    };
    // The business the customer buys as, and what the VAT registry last said.
    let company = load_company_identity(&services.executor, &id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|company| company.is_company())
        .map(|company| {
            let standing = match &company.last_check {
                None => "numéro jamais vérifié".to_owned(),
                Some(answer) if answer.valid => format!(
                    "valide le {}{}",
                    date(answer.checked_at),
                    answer
                        .consultation_ref
                        .as_ref()
                        .map(|proof| format!(" — consultation {proof}"))
                        .unwrap_or_default()
                ),
                Some(answer) => format!("inconnu du registre le {}", date(answer.checked_at)),
            };
            (company.company_name, company.vat_number, standing)
        });

    Ok(view! {
        page_header(parent: Section::Customers, title: &title)
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">(standing) (book.email.clone()) " · " <span class="font-mono text-xs">(id.clone())</span></p>

        detail_grid(
            detail_main(
                card(
                    card_header(card_title("Commandes"))
                    card_content(
                        if orders.is_empty() {
                            <p class="text-sm text-muted-foreground">"Aucune commande."</p>
                        } else {
                            <table class="w-full text-sm">
                                <tbody>
                                    for row in &orders {
                                        <tr class="border-b border-border last:border-0">
                                            <td class="py-2">(date(row.placed_at as u64))</td>
                                            <td class="py-2">link(href: href!(order_show, OrderId(row.order_id.clone())).resolve(cx), class: "font-mono text-xs", (row.order_number.clone().unwrap_or_else(|| row.order_id.clone())))</td>
                                            <td class="py-2">(row.status.clone())</td>
                                            <td class="py-2 text-right tabular-nums">(money(&Money::new(row.total_minor, &row.currency)))</td>
                                        </tr>
                                    }
                                </tbody>
                            </table>
                        }
                    )
                )
            )
            detail_side(
                if let Some((company_name, vat_number, standing)) = &company {
                    card(
                        card_header(card_title("Entreprise"))
                        card_content(
                            facts(
                                fact(term: "Raison sociale", (company_name.clone()))
                                fact(term: "Numéro de TVA", <span class="font-mono text-xs">(vat_number.clone())</span>)
                                fact(term: "Registre européen (VIES)", (standing.clone()))
                            )
                        )
                    )
                }
                card(
                    card_header(card_title("Adresse de facturation"))
                    card_content(
                        match &book.billing {
                            Some(address) => address_block(address: address),
                            None => <p class="text-sm text-muted-foreground">"Aucune."</p>,
                        }
                    )
                )
                card(
                    card_header(card_title("Adresses de livraison"))
                    card_content(
                        if book.deliveries.is_empty() {
                            <p class="text-sm text-muted-foreground">"Aucune."</p>
                        } else {
                            <ul class="flex flex-col gap-3">
                                for delivery in &book.deliveries {
                                    <li>
                                        address_block(address: &delivery.address)
                                        if delivery.preferred { <span class="text-xs text-muted-foreground">"Adresse préférée"</span> }
                                    </li>
                                }
                            </ul>
                        }
                    )
                )
            )
        )
    })
}

#[topcoat::view::component]
async fn address_block(address: &Address) -> Result<impl View> {
    Ok(view! {
        <address class="text-sm not-italic">
            <span class="block font-medium">(address.full_name())</span>
            <span class="block">(address.line1.clone())</span>
            if let Some(line2) = &address.line2 { <span class="block">(line2.clone())</span> }
            <span class="block">(address.postal_code.clone()) " " (address.city.clone()) ", " (address.country_code.clone())</span>
            if let Some(phone) = &address.phone { <span class="block text-muted-foreground">(phone.clone())</span> }
        </address>
    })
}
