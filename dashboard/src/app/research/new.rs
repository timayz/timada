//! `/research/new`: any signed-in user starts a study and owns it. The form
//! offers the countries and languages the keyword source has keywords for,
//! and nothing else: a study in a place it does not cover could never be
//! fetched.

use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, attributes, view},
};

use super::{error_code, research, study_id};
use crate::auth::require_user;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::button;
use crate::components::card::CARD;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::textarea::textarea;
use crate::keywords::{Keywords, Place};
use crate::ui::{Shell, describe_error, document, icons, notice, page_header};

#[topcoat::router::query_params(error = bad_request)]
struct NewQuery {
    error: Option<String>,
}

#[derive(serde::Serialize)]
struct NewLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

pub fn new_study_url(cx: &Cx, error: Option<&str>) -> String {
    href!(new).query(NewLink { error }).resolve(cx)
}

/// The place a new study is searched in unless its owner picks another.
const DEFAULT_PLACE: &str = "FR:fr";

/// One choice of the form's list: a country in one of its languages.
struct PlaceOption {
    /// `FR:fr`, as the form posts it.
    value: String,
    /// `France · French`.
    label: String,
}

/// What the form offers, by country name; or the code of why it offers
/// nothing.
async fn place_options(keywords: &Keywords) -> std::result::Result<Vec<PlaceOption>, &'static str> {
    let places = places(keywords).await?;
    let mut options: Vec<PlaceOption> = places
        .iter()
        .flat_map(|place| {
            place.languages.iter().map(|language| PlaceOption {
                value: format!("{}:{}", place.country, language.code),
                label: format!("{} · {}", place.name, language.name),
            })
        })
        .collect();
    options.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(options)
}

/// The places the source covers; or the code of why none can be told.
async fn places(keywords: &Keywords) -> std::result::Result<Vec<Place>, &'static str> {
    let Some(source) = &keywords.0 else {
        return Err("keywords_disabled");
    };
    source.places().await.map_err(|err| {
        tracing::warn!(error = %err, "the keyword source's countries could not be read");
        "keyword_places_unavailable"
    })
}

#[page]
pub async fn new(cx: &Cx) -> Result<impl View> {
    require_user(cx).await?;
    let query = query_params::<NewQuery>(cx)?;
    let options = place_options(context::<Keywords>(cx)).await;
    let unavailable = options.as_ref().err().map(|code| describe_error(code));
    let options = options.unwrap_or_default();
    Ok(view! {
        document(
            title: "Start a study",
            shell: Shell::page("research"),
            page_header(
                title: "Start a study",
                subtitle: Some("Name the niche and give the keywords it starts from. Nothing is fetched yet: you ask for the keywords on the study's page."),
            )
            notice(error: query.error.as_deref())
            if let Some(unavailable) = unavailable {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" data-research-disabled="" },
                    icon(data: icons::WARNING)
                    alert_description((unavailable))
                )
            } else {
                <form method="post" action=(href!(create)) class=(topcoat::view::class!(CARD, "gap-4 px-6"))>
                    field(
                        field_label(attrs: attributes! { for="study-name" }, "Name")
                        input(attrs: attributes! { id="study-name" name="name" required="" maxlength="80" placeholder="Cat trees" })
                    )
                    field(
                        field_label(attrs: attributes! { for="study-seeds" }, "Seed keywords, one a line")
                        textarea(attrs: attributes! { id="study-seeds" name="seeds" required="" rows="4" aria-describedby="study-seeds-hint" placeholder="arbre à chat" })
                        <p id="study-seeds-hint" class="text-sm text-muted-foreground">
                            "What a shopper would type. Each seed is fetched with the keywords that contain its words, up to 1,000 of them; a study holds 20 seeds at most."
                        </p>
                    )
                    field(
                        field_label(attrs: attributes! { for="study-place" }, "Searched in")
                        select(
                            attrs: attributes! { id="study-place" name="place" required="" },
                            for option in &options {
                                <option value=(&option.value) selected=(option.value == DEFAULT_PLACE)>(&option.label)</option>
                            }
                        )
                    )
                    <div>
                        button(attrs: attributes! { type="submit" }, "Start the study")
                    </div>
                </form>
            }
        )
    })
}

#[derive(serde::Deserialize)]
pub struct NewForm {
    name: String,
    seeds: String,
    /// `FR:fr`.
    place: String,
}

#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<NewForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    // Only a place the source covers: one typed by hand could never be
    // fetched.
    let places = match places(context::<Keywords>(cx)).await {
        Ok(places) => places,
        Err(code) => return Ok(see_other(new_study_url(cx, Some(code)))),
    };
    let Some((country, language)) = form.place.split_once(':').filter(|(country, language)| {
        places.iter().any(|place| {
            place.country == *country && place.languages.iter().any(|l| l.code == *language)
        })
    }) else {
        return Ok(see_other(new_study_url(cx, Some("invalid_study_place"))));
    };
    let input = timada_research::CreateStudy {
        name: form.name.clone(),
        seeds: form.seeds.lines().map(str::to_owned).collect(),
        country: country.to_owned(),
        language: language.to_owned(),
    };
    match research(cx).create_study(&user.id, input).await {
        Ok(study) => Ok(see_other(study_id::study_url(
            cx,
            &study.id,
            None,
            Some("Study started."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(new_study_url(cx, Some(error_code(&err)))))
        }
        Err(err) => Err(err.into()),
    }
}
