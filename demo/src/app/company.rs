//! `/account/company`: buying as a business — the company's name and VAT
//! number, checked against the VAT registry when given, and what the registry
//! last said. A valid number is what lets a business of another member state
//! buy without the shop's VAT.

use serde::Deserialize;
use timada_customer::{CompanyIdentityView, CustomerError, load_company_identity};
use timada_tax::{VatNumber, VatNumberError};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::RouterErrorExt, error::see_other, href, page},
    view::{View, component, view},
};

use super::{account, document, format::date};
use crate::{Store, auth::require_account};

#[derive(Debug, Deserialize)]
pub struct CompanyForm {
    company_name: String,
    vat_number: String,
}

#[page("/account/company")]
pub async fn show() -> Result<impl View> {
    Ok(view! { company_view(error: None, typed: None) })
}

fn refusal(err: &VatNumberError) -> String {
    match err {
        VatNumberError::MissingPrefix => {
            "Un numéro de TVA commence par les deux lettres de son pays : FR, DE, ES…".to_owned()
        }
        VatNumberError::UnknownPrefix(prefix) => {
            format!("« {prefix} » n'est pas le préfixe de TVA d'un État membre de l'Union.")
        }
        VatNumberError::Malformed(_) => {
            "Ce numéro n'a pas la forme d'un numéro de TVA de ce pays. Vérifiez-le.".to_owned()
        }
    }
}

/// Records the company, then asks the registry about its number straight
/// away; a registry that cannot answer now will be asked again at checkout.
#[page(POST "/account/company")]
pub async fn identify(cx: &Cx, Form(form): Form<CompanyForm>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let customers = timada_customer::Command(&store.executor);
    let error = match VatNumber::parse(&form.vat_number) {
        Err(err) => refusal(&err),
        Ok(number) => {
            match customers
                .identify_company(&account.customer_id, &form.company_name, &number)
                .await
            {
                Ok(()) => {
                    customers
                        .check_company_vat_number(
                            &account.customer_id,
                            store.vat_validator.as_ref(),
                        )
                        .await?;
                    return Err(see_other(href!(show).resolve(cx)).into());
                }
                Err(CustomerError::Required(_)) => {
                    "Indiquez la raison sociale de l'entreprise.".to_owned()
                }
                Err(err) => return Err(err.into()),
            }
        }
    };
    Ok(
        view! { company_view(error: Some(error), typed: Some((form.company_name, form.vat_number))) },
    )
}

/// Asks the registry again — after an outage, or once a new registration
/// went through.
#[page(POST "/account/company/check")]
pub async fn recheck(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    timada_customer::Command(&store.executor)
        .check_company_vat_number(&account.customer_id, store.vat_validator.as_ref())
        .await?;
    Err::<(), _>(see_other(href!(show).resolve(cx)).into())
}

#[page(POST "/account/company/remove")]
pub async fn remove(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    timada_customer::Command(&store.executor)
        .remove_company_identity(&account.customer_id)
        .await?;
    Err::<(), _>(see_other(href!(show).resolve(cx)).into())
}

/// What the page says of the number's standing.
fn standing(company: &CompanyIdentityView) -> (&'static str, String) {
    match &company.last_check {
        None => (
            "muted",
            "Ce numéro n'a pas encore pu être vérifié auprès du registre européen (VIES). \
             Nous réessaierons lors de votre commande."
                .to_owned(),
        ),
        Some(check) if check.valid => {
            let proof = check
                .consultation_ref
                .as_ref()
                .map(|reference| format!(", consultation n° {reference}"))
                .unwrap_or_default();
            let name = check
                .registered_name
                .as_ref()
                .map(|name| format!(" — enregistré au nom de {name}"))
                .unwrap_or_default();
            (
                "notice",
                format!(
                    "Numéro valide, vérifié le {}{proof}{name}.",
                    date(check.checked_at)
                ),
            )
        }
        Some(check) => (
            "error",
            format!(
                "Le registre européen (VIES) ne connaît pas ce numéro (vérifié le {}). \
                 Vos commandes sont facturées avec la TVA.",
                date(check.checked_at)
            ),
        ),
    }
}

#[component]
async fn company_view(
    cx: &Cx,
    error: Option<String>,
    typed: Option<(String, String)>,
) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let company = load_company_identity(&store.executor, &account.customer_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_not_found()?;
    let (name, number) =
        typed.unwrap_or_else(|| (company.company_name.clone(), company.vat_number.clone()));
    let is_company = company.is_company();
    let (standing_class, standing_text) = standing(&company);

    Ok(view! {
        document(
            title: "Compte professionnel",
            <h1>"Compte professionnel"</h1>
            <p>"Vous achetez pour une entreprise ? Sa raison sociale et son numéro de TVA figureront sur vos factures. Une entreprise d'un autre État membre de l'Union dont le numéro est valide est livrée hors TVA : elle l'autoliquide dans son pays."</p>
            if is_company {
                <p role="status" class=(standing_class)>(standing_text)</p>
            }
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            <form method="post" action=(href!(identify)) class="stack">
                <label>"Raison sociale"
                    <input name="company_name" required=(true) autocomplete="organization" value=(name)>
                </label>
                <label>"Numéro de TVA intracommunautaire"
                    <input name="vat_number" required=(true) autocomplete="off" placeholder="FR40303265045" value=(number)>
                </label>
                <button type="submit">"Enregistrer"</button>
            </form>
            if is_company {
                <form method="post" action=(href!(recheck)) class="inline">
                    <button type="submit" class="link">"Vérifier à nouveau le numéro"</button>
                </form>
                " · "
                <form method="post" action=(href!(remove)) class="inline">
                    <button type="submit" class="link">"Ne plus acheter en tant qu'entreprise"</button>
                </form>
            }
            <p><a href=(href!(account::overview))>"Retour à mon compte"</a></p>
        )
    })
}
