//! Ordering without an account. `/checkout/guest` asks who is ordering and
//! where it goes, registers them as a guest — a customer with nothing to sign
//! in to — and hands them to `/checkout`, which prices the order for that
//! address. `/order/{order_id}` is where a guest reads an order afterwards:
//! opened by the signed link of their e-mails, then by this browser's cookie.

use serde::Deserialize;
use timada_core::Address;
use timada_customer::{CustomerError, RegisterCustomer, load_address_book};
use timada_order::load_order_details;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{RouterErrorExt, see_other},
        href, page, path_param, path_param as param, query_params, query_params as query,
        response::response_headers,
    },
    view::{View, component, view},
};

use super::{account, cart, checkout, document};
use crate::{
    Store,
    auth::current_account,
    cart_session::current_cart,
    guest::{current_guest, current_shopper, order_key_is_valid, remember_guest},
};

#[derive(Debug, Deserialize)]
pub struct GuestForm {
    email: String,
    civility: String,
    first_name: String,
    last_name: String,
    line1: String,
    line2: Option<String>,
    postal_code: String,
    city: String,
    country_code: String,
    phone: Option<String>,
}

impl GuestForm {
    fn address(&self) -> Address {
        let optional = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        Address {
            civility: account::civility(&self.civility),
            first_name: self.first_name.trim().to_owned(),
            last_name: self.last_name.trim().to_owned(),
            line1: self.line1.trim().to_owned(),
            line2: optional(&self.line2),
            postal_code: self.postal_code.trim().to_owned(),
            city: self.city.trim().to_owned(),
            country_code: self.country_code.trim().to_uppercase(),
            phone: optional(&self.phone),
            mobile: None,
        }
    }
}

/// Somebody signed in orders with their account; an empty cart orders nothing.
async fn nothing_to_do_here(cx: &Cx) -> Result<()> {
    let signed_in = current_account(cx)
        .await
        .map_err(|err| anyhow::anyhow!("{err:#}"))?
        .is_some();
    if signed_in {
        return Err(see_other(href!(checkout::show).resolve(cx)).into());
    }
    match current_cart(cx).await {
        Ok(Some(cart)) if !cart.lines.is_empty() => Ok(()),
        Ok(_) => Err(see_other(href!(cart::show).resolve(cx)).into()),
        Err(err) => Err(anyhow::anyhow!("{err:#}").into()),
    }
}

#[page("/checkout/guest")]
pub async fn show(cx: &Cx) -> Result<impl View> {
    nothing_to_do_here(cx).await?;
    // Back from the checkout to correct something: what was said is shown.
    let store = app_context::<Store>(cx);
    let known = match current_guest(cx).await {
        Ok(Some(customer_id)) => load_address_book(&store.executor, customer_id).await?,
        Ok(None) => None,
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    let (email, address) = match known {
        Some(book) => {
            let address = book
                .preferred_delivery()
                .or(book.deliveries.last())
                .map(|delivery| delivery.address.clone());
            (book.email, address)
        }
        None => (String::new(), None),
    };
    let address = address.unwrap_or_else(|| Address {
        country_code: "FR".to_owned(),
        ..Address::default()
    });
    Ok(view! { guest_form_view(email: email, address: &address, error: None) })
}

#[page(POST "/checkout/guest")]
pub async fn submit(cx: &Cx, Form(form): Form<GuestForm>) -> Result<impl View> {
    nothing_to_do_here(cx).await?;
    let address = form.address();
    match register(cx, &form, &address).await? {
        Ok(customer_id) => {
            remember_guest(cx, &customer_id);
            Err(see_other(href!(checkout::show).resolve(cx)).into())
        }
        Err(error) => Ok(view! {
            guest_form_view(email: form.email.trim().to_owned(), address: &address, error: Some(error))
        }),
    }
}

/// The guest these details are of, with that address as the one to deliver
/// to; `Ok(Err(message))` is a refusal to show on the form.
///
/// The guest this browser registered a moment ago is kept when they are the
/// same person — they came back to correct the address; anybody else is a new
/// guest. An address an account already uses is no obstacle, and is not
/// remarked upon: a guest claims nothing.
async fn register(
    cx: &Cx,
    form: &GuestForm,
    address: &Address,
) -> Result<std::result::Result<String, String>> {
    // Before anybody is registered: a form that is going to be refused
    // must not leave a customer behind.
    if let Err(err) = address.validate() {
        return Ok(Err(format!("Adresse incomplète : {err}.")));
    }
    let store = app_context::<Store>(cx);
    let customers = timada_customer::Command(&store.executor);
    let known = match current_guest(cx).await {
        Ok(Some(customer_id)) => load_address_book(&store.executor, customer_id).await?,
        Ok(None) => None,
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    let same_person = |book: &timada_customer::AddressBookView| {
        book.email == form.email.trim().to_lowercase()
            && book.civility == address.civility
            && book.first_name == address.first_name
            && book.last_name == address.last_name
    };
    let customer_id = match known.filter(|book| same_person(book)) {
        Some(book) => book.customer_id,
        None => {
            let registered = customers
                .register_guest(RegisterCustomer {
                    email: form.email.clone(),
                    civility: address.civility.clone(),
                    first_name: address.first_name.clone(),
                    last_name: address.last_name.clone(),
                })
                .await;
            match registered {
                Ok(customer_id) => customer_id,
                Err(CustomerError::InvalidEmail(_)) => {
                    return Ok(Err(
                        "Indiquez une adresse e-mail valide : la confirmation et \
                                   le suivi de votre commande y sont envoyés."
                            .to_owned(),
                    ));
                }
                Err(CustomerError::Required(_)) => {
                    return Ok(Err("Indiquez votre prénom et votre nom.".to_owned()));
                }
                Err(err) => return Err(anyhow::Error::from(err).into()),
            }
        }
    };
    match customers
        .add_delivery_address(&customer_id, address.clone())
        .await
    {
        Ok(address_id) => {
            customers
                .choose_preferred_delivery_address(&customer_id, address_id)
                .await
                .map_err(anyhow::Error::from)?;
            Ok(Ok(customer_id))
        }
        Err(CustomerError::Address(err)) => Ok(Err(format!("Adresse incomplète : {err}."))),
        Err(err) => Err(anyhow::Error::from(err).into()),
    }
}

#[component]
async fn guest_form_view(
    cx: &Cx,
    email: String,
    address: &Address,
    error: Option<String>,
) -> Result<impl View> {
    let login = href!(account::login)
        .query([("next", href!(checkout::show).resolve(cx))])
        .resolve(cx);
    let action = href!(submit).resolve(cx);
    Ok(view! {
        document(
            title: "Commander",
            <h1>"Commander"</h1>
            <p>"Vous avez un compte ? " <a href=(login)>"Connectez-vous"</a> " pour retrouver vos adresses et vos commandes."</p>
            <h2>"Commander sans compte"</h2>
            <p class="muted">"Dites-nous où livrer et où vous écrire : le suivi de la commande et sa facture vous sont envoyés par e-mail. Vous pourrez créer un compte ensuite, si vous le souhaitez."</p>
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            <form method="post" action=(action) class="stack">
                <label for="email">"Adresse e-mail" <input id="email" name="email" type="email" required=(true) autocomplete="email" value=(email)></label>
                account::civility_select(selected: &address.civility)
                <label for="first_name">"Prénom" <input id="first_name" name="first_name" required=(true) autocomplete="given-name" value=(address.first_name.clone())></label>
                <label for="last_name">"Nom" <input id="last_name" name="last_name" required=(true) autocomplete="family-name" value=(address.last_name.clone())></label>
                <label for="line1">"Adresse de livraison" <input id="line1" name="line1" required=(true) autocomplete="address-line1" value=(address.line1.clone())></label>
                <label for="line2">"Complément d'adresse (facultatif)" <input id="line2" name="line2" autocomplete="address-line2" value=(address.line2.clone().unwrap_or_default())></label>
                <label for="postal_code">"Code postal" <input id="postal_code" name="postal_code" required=(true) autocomplete="postal-code" value=(address.postal_code.clone())></label>
                <label for="city">"Ville" <input id="city" name="city" required=(true) autocomplete="address-level2" value=(address.city.clone())></label>
                <label for="country_code">"Pays (code ISO à deux lettres, ex. FR, MQ)"
                    <input id="country_code" name="country_code" required=(true) minlength="2" maxlength="2" autocomplete="country" value=(address.country_code.clone())>
                </label>
                <label for="phone">"Téléphone (facultatif)" <input id="phone" name="phone" type="tel" autocomplete="tel" value=(address.phone.clone().unwrap_or_default())></label>
                <button type="submit">"Continuer vers la livraison et le paiement"</button>
            </form>
        )
    })
}

path_param!(pub order_id: String, error = not_found);

/// `?cle=<key>`: the signature of the link written in the e-mails.
#[query_params(error = bad_request)]
struct OrderQuery {
    cle: Option<String>,
}

/// A guest's order. The signed link makes this browser the guest's — then
/// leaves for the same address without the key, which stays out of the
/// history and of what the page tells other sites. Once the guest has an
/// account, the link only leads to it: they sign in like anybody else.
#[page("/order/{order_id}")]
pub async fn order(cx: &Cx) -> Result<impl View> {
    let id = param::<OrderId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    if let Some(key) = &query::<OrderQuery>(cx)?.cle {
        order_key_is_valid(&store.link_secret, &id, key)
            .then_some(())
            .ok_or_not_found()?;
        let order = load_order_details(&store.executor, &id)
            .await?
            .ok_or_not_found()?;
        let still_a_guest = load_address_book(&store.executor, &order.customer_id)
            .await?
            .is_some_and(|customer| customer.guest);
        if !still_a_guest {
            let theirs = href!(account::order_detail, checkout::OrderId(id)).resolve(cx);
            return Err(see_other(theirs).into());
        }
        remember_guest(cx, &order.customer_id);
        response_headers(cx).append(
            topcoat::router::header::REFERRER_POLICY,
            topcoat::router::header::HeaderValue::from_static("no-referrer"),
        );
        return Err(see_other(href!(order, OrderId(id)).resolve(cx)).into());
    }
    match current_shopper(cx).await? {
        Some(shopper) if shopper.guest => Ok(view! {
            account::order_page(id: id, customer_id: shopper.customer_id, guest: true)
        }),
        // An account reads its orders in its account.
        Some(_) => {
            let theirs = href!(account::order_detail, checkout::OrderId(id)).resolve(cx);
            Err(see_other(theirs).into())
        }
        None => Err(topcoat::router::error::not_found().into()),
    }
}
