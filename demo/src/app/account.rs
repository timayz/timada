//! Sign-up, login and "mon compte": identity, the address book and the
//! order history. Every page below `/account` requires a signed-in shopper.

use serde::Deserialize;
use timada_core::{Address, Civility};
use timada_customer::{
    AddressBookView, CustomerError, DeliveryAddress, RegisterCustomer, load_address_book,
};
use timada_order::{OrderDetailsView, PaymentMode, load_order_details, orders_of_customer};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{RouterErrorExt, see_other},
        href, page, path_param, path_param as param, query_params, query_params as query,
    },
    view::{View, component, view},
};

use super::{
    catalog,
    checkout::OrderId,
    document,
    format::{address_lines, date, money, order_status},
    safe_next,
};
use crate::{
    Store,
    auth::{self, SignUpError, require_account},
    cart_session::forget_cart,
};

#[query_params(error = bad_request)]
struct NextQuery {
    next: Option<String>,
}

// ---------------------------------------------------------------- sign-up

#[derive(Debug, Deserialize)]
pub struct RegisterForm {
    civility: String,
    first_name: String,
    last_name: String,
    email: String,
    password: String,
    next: Option<String>,
}

#[page("/register")]
pub async fn register(cx: &Cx) -> Result<impl View> {
    let next = query::<NextQuery>(cx)?.next.clone();
    Ok(view! { register_view(next: next, error: None) })
}

#[page(POST "/register")]
pub async fn register_submit(cx: &Cx, Form(form): Form<RegisterForm>) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    let signed_up = auth::sign_up(
        store,
        RegisterCustomer {
            email: form.email,
            civility: civility(&form.civility),
            first_name: form.first_name.trim().to_owned(),
            last_name: form.last_name.trim().to_owned(),
        },
        &form.password,
    )
    .await;
    match signed_up {
        Ok(account) => {
            auth::start_session(cx, &account).await?;
            let target = safe_next(form.next).unwrap_or_else(|| href!(overview).resolve(cx));
            Err(see_other(target).into())
        }
        Err(SignUpError::Server(err)) => Err(err.into()),
        Err(err) => Ok(view! { register_view(next: form.next, error: Some(err.to_string())) }),
    }
}

#[component]
async fn register_view(cx: &Cx, next: Option<String>, error: Option<String>) -> Result<impl View> {
    let login_link = href!(login)
        .query([("next", next.clone().unwrap_or_default())])
        .resolve(cx);
    Ok(view! {
        document(
            title: "Créer un compte",
            <h1>"Créer un compte"</h1>
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            <form method="post" action=(href!(register_submit)) class="stack">
                if let Some(next) = &next { <input type="hidden" name="next" value=(next.clone())> }
                civility_select(selected: &Civility::Mr)
                <label for="first_name">"Prénom" <input id="first_name" name="first_name" required=(true) autocomplete="given-name"></label>
                <label for="last_name">"Nom" <input id="last_name" name="last_name" required=(true) autocomplete="family-name"></label>
                <label for="email">"Email" <input id="email" name="email" type="email" required=(true) autocomplete="email"></label>
                <label for="password">"Mot de passe (8 caractères minimum)"
                    <input id="password" name="password" type="password" minlength="8" required=(true) autocomplete="new-password">
                </label>
                <button type="submit">"Créer mon compte"</button>
            </form>
            <p>"Déjà client ? " <a href=(login_link)>"Se connecter"</a></p>
        )
    })
}

// ------------------------------------------------------------------ login

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
    next: Option<String>,
}

#[page("/login")]
pub async fn login(cx: &Cx) -> Result<impl View> {
    let next = query::<NextQuery>(cx)?.next.clone();
    Ok(view! { login_view(next: next, error: None) })
}

#[page(POST "/login")]
pub async fn login_submit(cx: &Cx, Form(form): Form<LoginForm>) -> Result<impl View> {
    if auth::sign_in(cx, &form.email, &form.password).await? {
        let target = safe_next(form.next).unwrap_or_else(|| href!(overview).resolve(cx));
        return Err(see_other(target).into());
    }
    Ok(view! { login_view(next: form.next, error: Some("Email ou mot de passe incorrect.")) })
}

/// Closes the session and forgets the cart, which may belong to the account.
#[page(POST "/logout")]
pub async fn logout(cx: &Cx) -> Result<impl View> {
    auth::sign_out(cx).await?;
    forget_cart(cx);
    Err::<(), _>(see_other(href!(catalog::home).resolve(cx)).into())
}

#[component]
async fn login_view(cx: &Cx, next: Option<String>, error: Option<&str>) -> Result<impl View> {
    let register_link = href!(register)
        .query([("next", next.clone().unwrap_or_default())])
        .resolve(cx);
    Ok(view! {
        document(
            title: "Connexion",
            <h1>"Connexion"</h1>
            if let Some(error) = error { <p role="alert" class="error">(error)</p> }
            <form method="post" action=(href!(login_submit)) class="stack">
                if let Some(next) = &next { <input type="hidden" name="next" value=(next.clone())> }
                <label for="email">"Email" <input id="email" name="email" type="email" required=(true) autocomplete="username"></label>
                <label for="password">"Mot de passe" <input id="password" name="password" type="password" required=(true) autocomplete="current-password"></label>
                <button type="submit">"Se connecter"</button>
            </form>
            <p>"Nouveau client ? " <a href=(register_link)>"Créer un compte"</a></p>
        )
    })
}

// ---------------------------------------------------------------- account

async fn address_book(cx: &Cx) -> Result<AddressBookView> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    Ok(load_address_book(&store.executor, &account.customer_id)
        .await?
        .ok_or_not_found()?)
}

#[page("/account")]
pub async fn overview(cx: &Cx) -> Result<impl View> {
    let book = address_book(cx).await?;
    Ok(view! {
        document(
            title: "Mon compte",
            <h1>"Mon compte"</h1>
            <p>(book.first_name.clone()) " " (book.last_name.clone()) " · " (book.email.clone())</p>
            <ul>
                <li><a href=(href!(orders))>"Historique de mes commandes"</a></li>
                <li><a href=(href!(addresses))>"Mes adresses"</a></li>
            </ul>
        )
    })
}

// -------------------------------------------------------------- addresses

#[page("/account/addresses")]
pub async fn addresses(cx: &Cx) -> Result<impl View> {
    let book = address_book(cx).await?;
    Ok(view! { addresses_view(book: &book, error: None) })
}

#[component]
async fn addresses_view(book: &AddressBookView, error: Option<String>) -> Result<impl View> {
    Ok(view! {
        document(
            title: "Mes adresses",
            <h1>"Mes adresses"</h1>
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            <h2>"Adresse de facturation"</h2>
            match &book.billing {
                Some(billing) => {
                    <div class="card">
                        address_block(address: billing)
                        <p><a href=(href!(billing_address))>"Modifier"</a></p>
                    </div>
                }
                None => <p class="muted">"Aucune. " <a href=(href!(billing_address))>"Renseigner mon adresse de facturation"</a></p>,
            }
            <h2>"Adresses de livraison"</h2>
            if book.deliveries.is_empty() { <p class="muted">"Aucune adresse de livraison."</p> }
            <div class="cards">
                for delivery in &book.deliveries { delivery_card(delivery: delivery) }
            </div>
            <p><a href=(href!(new_address))>"Ajouter une adresse de livraison"</a></p>
        )
    })
}

#[component]
async fn address_block(address: &Address) -> Result<impl View> {
    Ok(view! {
        <address>
            for line in address_lines(address) { (line) <br> }
        </address>
    })
}

#[component]
async fn delivery_card(cx: &Cx, delivery: &DeliveryAddress) -> Result<impl View> {
    let edit = href!(edit_address, AddressId(delivery.id.clone())).resolve(cx);
    let prefer = href!(prefer_address, AddressId(delivery.id.clone())).resolve(cx);
    let remove = href!(remove_address, AddressId(delivery.id.clone())).resolve(cx);
    let whose = format!("{}, {}", delivery.address.line1, delivery.address.city);
    Ok(view! {
        <div class="card">
            if delivery.preferred { <p><strong>"Adresse préférée"</strong></p> }
            address_block(address: &delivery.address)
            <p>
                <a href=(edit)>"Modifier" <span class="muted">" (" (whose.clone()) ")"</span></a>
            </p>
            if !delivery.preferred {
                <form method="post" action=(prefer) class="inline">
                    <button type="submit" class="link">"Définir comme préférée" <span class="muted">" (" (whose.clone()) ")"</span></button>
                </form>
                " "
            }
            <form method="post" action=(remove) class="inline">
                <button type="submit" class="link">"Supprimer" <span class="muted">" (" (whose) ")"</span></button>
            </form>
        </div>
    })
}

#[derive(Debug, Deserialize)]
pub struct AddressForm {
    civility: String,
    first_name: String,
    last_name: String,
    line1: String,
    line2: Option<String>,
    postal_code: String,
    city: String,
    country_code: String,
    phone: Option<String>,
    mobile: Option<String>,
    next: Option<String>,
}

impl AddressForm {
    fn address(&self) -> Address {
        let optional = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        Address {
            civility: civility(&self.civility),
            first_name: self.first_name.trim().to_owned(),
            last_name: self.last_name.trim().to_owned(),
            line1: self.line1.trim().to_owned(),
            line2: optional(&self.line2),
            postal_code: self.postal_code.trim().to_owned(),
            city: self.city.trim().to_owned(),
            country_code: self.country_code.trim().to_uppercase(),
            phone: optional(&self.phone),
            mobile: optional(&self.mobile),
        }
    }
}

fn civility(value: &str) -> Civility {
    match value {
        "mrs" => Civility::Mrs,
        _ => Civility::Mr,
    }
}

/// A refused address is shown on the form; anything else is a 500.
fn address_outcome<T>(
    result: std::result::Result<T, CustomerError>,
) -> Result<std::result::Result<T, String>> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(CustomerError::Address(err)) => Ok(Err(format!("Adresse incomplète : {err}."))),
        Err(CustomerError::AddressNotFound) => Ok(Err("Cette adresse n'existe plus.".to_owned())),
        Err(CustomerError::CannotRemovePreferred) => Ok(Err(
            "Choisissez une autre adresse préférée avant de supprimer celle-ci.".to_owned(),
        )),
        Err(err) => Err(anyhow::Error::from(err).into()),
    }
}

#[page("/account/addresses/new")]
pub async fn new_address(cx: &Cx) -> Result<impl View> {
    let book = address_book(cx).await?;
    let next = query::<NextQuery>(cx)?.next.clone();
    let prefilled = Address {
        civility: book.civility.clone(),
        first_name: book.first_name.clone(),
        last_name: book.last_name.clone(),
        country_code: "FR".to_owned(),
        ..Address::default()
    };
    let action = href!(create_address).resolve(cx);
    Ok(view! {
        address_form_view(title: "Nouvelle adresse de livraison", action: action, address: &prefilled, next: next, error: None)
    })
}

#[page(POST "/account/addresses/new")]
pub async fn create_address(cx: &Cx, Form(form): Form<AddressForm>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let address = form.address();
    let added = address_outcome(
        timada_customer::Command(&store.executor)
            .add_delivery_address(&account.customer_id, address.clone())
            .await,
    )?;
    match added {
        Ok(_) => {
            let target = safe_next(form.next).unwrap_or_else(|| href!(addresses).resolve(cx));
            Err(see_other(target).into())
        }
        Err(error) => {
            let action = href!(create_address).resolve(cx);
            Ok(view! {
                address_form_view(title: "Nouvelle adresse de livraison", action: action, address: &address, next: form.next, error: Some(error))
            })
        }
    }
}

path_param!(address_id: String, error = not_found);

#[page("/account/addresses/{address_id}/edit")]
pub async fn edit_address(cx: &Cx) -> Result<impl View> {
    let book = address_book(cx).await?;
    let id = param::<AddressId>(cx)?.clone();
    let address = book
        .deliveries
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.address.clone())
        .ok_or_not_found()?;
    let action = href!(update_address, AddressId(id)).resolve(cx);
    Ok(view! {
        address_form_view(title: "Modifier l'adresse de livraison", action: action, address: &address, next: None, error: None)
    })
}

#[page(POST "/account/addresses/{address_id}/edit")]
pub async fn update_address(cx: &Cx, Form(form): Form<AddressForm>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<AddressId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let address = form.address();
    let changed = address_outcome(
        timada_customer::Command(&store.executor)
            .change_delivery_address(&account.customer_id, id.clone(), address.clone())
            .await,
    )?;
    match changed {
        Ok(()) => Err(see_other(href!(addresses).resolve(cx)).into()),
        Err(error) => {
            let action = href!(update_address, AddressId(id)).resolve(cx);
            Ok(view! {
                address_form_view(title: "Modifier l'adresse de livraison", action: action, address: &address, next: None, error: Some(error))
            })
        }
    }
}

#[page(POST "/account/addresses/{address_id}/prefer")]
pub async fn prefer_address(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<AddressId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let outcome = address_outcome(
        timada_customer::Command(&store.executor)
            .choose_preferred_delivery_address(&account.customer_id, id)
            .await,
    )?;
    match outcome {
        Ok(()) => Err(see_other(href!(addresses).resolve(cx)).into()),
        Err(error) => {
            let book = address_book(cx).await?;
            Ok(view! { addresses_view(book: &book, error: Some(error)) })
        }
    }
}

#[page(POST "/account/addresses/{address_id}/remove")]
pub async fn remove_address(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<AddressId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let outcome = address_outcome(
        timada_customer::Command(&store.executor)
            .remove_delivery_address(&account.customer_id, id)
            .await,
    )?;
    match outcome {
        Ok(()) => Err(see_other(href!(addresses).resolve(cx)).into()),
        Err(error) => {
            let book = address_book(cx).await?;
            Ok(view! { addresses_view(book: &book, error: Some(error)) })
        }
    }
}

#[page("/account/billing")]
pub async fn billing_address(cx: &Cx) -> Result<impl View> {
    let book = address_book(cx).await?;
    let current = book.billing.clone().unwrap_or_else(|| Address {
        civility: book.civility.clone(),
        first_name: book.first_name.clone(),
        last_name: book.last_name.clone(),
        country_code: "FR".to_owned(),
        ..Address::default()
    });
    let action = href!(set_billing_address).resolve(cx);
    Ok(view! {
        address_form_view(title: "Adresse de facturation", action: action, address: &current, next: None, error: None)
    })
}

#[page(POST "/account/billing")]
pub async fn set_billing_address(cx: &Cx, Form(form): Form<AddressForm>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let address = form.address();
    let set = address_outcome(
        timada_customer::Command(&store.executor)
            .set_billing_address(&account.customer_id, address.clone())
            .await,
    )?;
    match set {
        Ok(()) => Err(see_other(href!(addresses).resolve(cx)).into()),
        Err(error) => {
            let action = href!(set_billing_address).resolve(cx);
            Ok(view! {
                address_form_view(title: "Adresse de facturation", action: action, address: &address, next: None, error: Some(error))
            })
        }
    }
}

#[component]
async fn civility_select(selected: &Civility) -> Result<impl View> {
    Ok(view! {
        <label for="civility">"Civilité"
            <select id="civility" name="civility" autocomplete="honorific-prefix">
                <option value="mr" selected=(*selected == Civility::Mr)>"M."</option>
                <option value="mrs" selected=(*selected == Civility::Mrs)>"Mme"</option>
            </select>
        </label>
    })
}

#[component]
async fn address_form_view(
    title: &str,
    action: String,
    address: &Address,
    next: Option<String>,
    error: Option<String>,
) -> Result<impl View> {
    Ok(view! {
        document(
            title: title,
            <h1>(title)</h1>
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            <form method="post" action=(action) class="stack">
                if let Some(next) = &next { <input type="hidden" name="next" value=(next.clone())> }
                civility_select(selected: &address.civility)
                <label for="first_name">"Prénom" <input id="first_name" name="first_name" required=(true) autocomplete="given-name" value=(address.first_name.clone())></label>
                <label for="last_name">"Nom" <input id="last_name" name="last_name" required=(true) autocomplete="family-name" value=(address.last_name.clone())></label>
                <label for="line1">"Adresse" <input id="line1" name="line1" required=(true) autocomplete="address-line1" value=(address.line1.clone())></label>
                <label for="line2">"Complément d'adresse (facultatif)" <input id="line2" name="line2" autocomplete="address-line2" value=(address.line2.clone().unwrap_or_default())></label>
                <label for="postal_code">"Code postal" <input id="postal_code" name="postal_code" required=(true) autocomplete="postal-code" value=(address.postal_code.clone())></label>
                <label for="city">"Ville" <input id="city" name="city" required=(true) autocomplete="address-level2" value=(address.city.clone())></label>
                <label for="country_code">"Pays (code ISO à deux lettres, ex. FR, MQ)"
                    <input id="country_code" name="country_code" required=(true) minlength="2" maxlength="2" autocomplete="country" value=(address.country_code.clone())>
                </label>
                <label for="phone">"Téléphone (facultatif)" <input id="phone" name="phone" type="tel" autocomplete="tel" value=(address.phone.clone().unwrap_or_default())></label>
                <label for="mobile">"Mobile (facultatif)" <input id="mobile" name="mobile" type="tel" autocomplete="tel" value=(address.mobile.clone().unwrap_or_default())></label>
                <button type="submit">"Enregistrer"</button>
            </form>
            <p><a href=(href!(addresses))>"Retour à mes adresses"</a></p>
        )
    })
}

// ----------------------------------------------------------------- orders

#[page("/account/orders")]
pub async fn orders(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let rows = orders_of_customer(&store.db, &account.customer_id).await?;
    let mut listed = Vec::with_capacity(rows.len());
    for row in &rows {
        listed.push((
            href!(order_detail, OrderId(row.order_id.clone())).resolve(cx),
            row.order_id.clone(),
            date(row.placed_at.max(0) as u64),
            order_status(&row.status),
            money(&timada_core::Money::new(row.total_minor, &row.currency)),
        ));
    }

    Ok(view! {
        document(
            title: "Historique de mes commandes",
            <h1>"Historique de mes commandes"</h1>
            if listed.is_empty() {
                <p class="muted">"Vous n'avez pas encore passé de commande."</p>
            } else {
                <table>
                    <caption class="muted">"Vos commandes, de la plus récente à la plus ancienne"</caption>
                    <thead>
                        <tr>
                            <th scope="col">"Commande"</th>
                            <th scope="col">"Date"</th>
                            <th scope="col">"Statut"</th>
                            <th scope="col" class="num">"Total"</th>
                        </tr>
                    </thead>
                    <tbody>
                        for (link, id, placed, status, total) in &listed {
                            <tr>
                                <th scope="row"><a href=(link.clone())>(id.clone())</a></th>
                                <td>(placed.clone())</td>
                                <td>(*status)</td>
                                <td class="num">(total.clone())</td>
                            </tr>
                        }
                    </tbody>
                </table>
            }
        )
    })
}

/// One order of the signed-in shopper; someone else's order is a 404.
#[page("/account/orders/{order_id}")]
pub async fn order_detail(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<OrderId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let order = load_order_details(&store.executor, &id)
        .await?
        .filter(|o| o.customer_id == account.customer_id)
        .ok_or_not_found()?;
    Ok(view! { order_view(order: &order) })
}

#[component]
async fn order_view(order: &OrderDetailsView) -> Result<impl View> {
    let payment = match order.payment_mode {
        PaymentMode::Card => "Carte bancaire".to_owned(),
        PaymentMode::Installments { count } => format!("Paiement en {count} fois"),
    };
    let mut line_totals = Vec::with_capacity(order.lines.len());
    for line in &order.lines {
        line_totals.push((line, money(&line.total()?)));
    }

    Ok(view! {
        document(
            title: "Détail de la commande",
            <h1>"Commande " (order.id.clone())</h1>
            <p>
                "Passée le " (date(order.placed_at)) " · "
                <strong>(order_status(order.status.as_str()))</strong>
            </p>
            if let Some(shipped_at) = order.shipped_at {
                <p>
                    "Expédiée le " (date(shipped_at))
                    if let Some(carrier) = &order.carrier { " par " (carrier.clone()) }
                    if let Some(tracking) = &order.tracking_number { " — suivi " (tracking.clone()) }
                </p>
            }
            if let Some(reason) = &order.cancelled_reason { <p>"Motif d'annulation : " (reason.clone())</p> }
            <table>
                <caption class="muted">"Articles commandés"</caption>
                <thead>
                    <tr>
                        <th scope="col">"Produit"</th>
                        <th scope="col" class="num">"Quantité"</th>
                        <th scope="col" class="num">"Prix unitaire"</th>
                        <th scope="col" class="num">"Total"</th>
                    </tr>
                </thead>
                <tbody>
                    for (line, total) in &line_totals {
                        <tr>
                            <th scope="row">(line.name.clone())</th>
                            <td class="num">(line.quantity.to_string())</td>
                            <td class="num">(money(&line.unit_price))</td>
                            <td class="num">(total.clone())</td>
                        </tr>
                    }
                </tbody>
            </table>
            <table class="totals">
                <tbody>
                    <tr><td>"Sous-total"</td><td class="num">(money(&order.subtotal))</td></tr>
                    <tr><td>"Livraison (" (order.delivery.method_code.clone()) ")"</td><td class="num">(money(&order.shipping_fee))</td></tr>
                    if order.handling_fee.is_positive() {
                        <tr><td>"Frais de dossier"</td><td class="num">(money(&order.handling_fee))</td></tr>
                    }
                    if let Some(discount) = &order.discount {
                        <tr><td>"Remise (" (discount.code.clone()) ")"</td><td class="num">"− " (money(&discount.amount))</td></tr>
                    }
                    <tr class="total"><td>"Total TTC"</td><td class="num">(money(&order.total))</td></tr>
                </tbody>
            </table>
            <p>"Paiement : " (payment)</p>
            <div class="cards">
                <div class="card"><h2>"Livraison"</h2> address_block(address: &order.delivery_address)</div>
                <div class="card"><h2>"Facturation"</h2> address_block(address: &order.billing_address)</div>
            </div>
            <p><a href=(href!(orders))>"Retour à mes commandes"</a></p>
        )
    })
}
