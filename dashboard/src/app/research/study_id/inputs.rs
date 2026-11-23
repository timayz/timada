use timada_catalog::parse_amount;
use timada_research::{Sale, SetInputs};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{require_owned, study_url};
use crate::app::research::{error_code, research};
use crate::ecb::EuroRates;
use crate::research::usd_worth;

/// What the owner says of the niche, as the page's form posts it; a field
/// left empty is nothing said.
#[derive(serde::Deserialize)]
pub struct InputsForm {
    currency: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    cost: String,
    #[serde(default)]
    shipping: String,
    #[serde(default)]
    import: String,
    #[serde(default)]
    vat_percent: String,
    #[serde(default)]
    fixed: String,
    #[serde(default)]
    fee_percent: String,
    #[serde(default)]
    fee_fixed: String,
    /// One a line.
    #[serde(default)]
    brands: String,
    /// One a line.
    #[serde(default)]
    excluded_words: String,
    #[serde(default)]
    supplier_products: String,
    /// `yes`, `no`, or empty for not looked at.
    #[serde(default)]
    specialist_first: String,
}

impl InputsForm {
    /// What the form says, or the code of the field that does not read.
    fn inputs(&self) -> std::result::Result<SetInputs, &'static str> {
        let said = |text: &str| match text.trim() {
            "" => Ok(None),
            text => parse_amount(text)
                .map(Some)
                .map_err(|_| "invalid_study_amount"),
        };
        let amount = |text: &str| said(text).map(Option::unwrap_or_default);
        let rate = |text: &str| {
            amount(text).and_then(|bps| u32::try_from(bps).map_err(|_| "invalid_study_rate"))
        };
        let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
        Ok(SetInputs {
            sale: Sale {
                currency: self.currency.clone(),
                price_cents: said(&self.price)?,
                cost_cents: said(&self.cost)?,
                shipping_cents: amount(&self.shipping)?,
                import_cents: amount(&self.import)?,
                vat_bps: rate(&self.vat_percent)?,
                fixed_cents: amount(&self.fixed)?,
                payment_fee_bps: rate(&self.fee_percent)?,
                payment_fee_cents: amount(&self.fee_fixed)?,
            },
            brands: lines(&self.brands),
            excluded_words: lines(&self.excluded_words),
            supplier_products: match self.supplier_products.trim() {
                "" => None,
                text => Some(
                    text.parse::<u32>()
                        .map_err(|_| "invalid_supplier_products")?,
                ),
            },
            specialist_first: match self.specialist_first.as_str() {
                "yes" => Some(true),
                "no" => Some(false),
                _ => None,
            },
            usd_micros: None,
        })
    }
}

/// Says what the study is scored from. Nothing is fetched: the score is
/// worked out again from the keywords the study holds. The day's rate of
/// the sale's currency goes with it, so a cost per click, which the source
/// gives in US dollars, is read in that currency.
#[route(POST)]
pub async fn set(cx: &Cx, Form(form): Form<InputsForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    let back = |error: Option<&str>, message: Option<&str>| {
        Ok(see_other(format!(
            "{}#score",
            study_url(cx, &study.id, error, message)
        )))
    };
    let mut input = match form.inputs() {
        Ok(input) => input,
        Err(code) => return back(Some(code), None),
    };
    let currency = input.sale.currency.trim().to_ascii_uppercase();
    let rates = context::<std::sync::Arc<dyn EuroRates>>(cx);
    let day = timada_core::civil::format_date(crate::auth::now());
    input.usd_micros = usd_worth(rates.as_ref(), &currency, &day).await;
    match research(cx).set_inputs(&user_id, &study.id, input).await {
        Ok(()) => back(None, Some("Saved: the study is scored again.")),
        Err(err) if err.is_user_facing() => back(Some(error_code(&err)), None),
        Err(err) => Err(err.into()),
    }
}
