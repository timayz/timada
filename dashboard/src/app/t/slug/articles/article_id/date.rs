//! `POST /t/{slug}/articles/{article_id}/date` (`date=YYYY-MM-DD`): the day
//! the article says it came out.

use serde::Deserialize;
use timada_pages::PageError;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;
use crate::ui::{format_date, parse_date};

#[derive(Deserialize)]
pub struct DateForm {
    date: String,
}

#[route(POST)]
pub async fn date(cx: &Cx, Form(form): Form<DateForm>) -> Result<SeeOther> {
    act(cx, "Date changed.", async |blog, actor, id| {
        let day = parse_date(&form.date).ok_or(PageError::InvalidArticleDate)?;
        // The day it has already: its hour stays.
        let dated = blog.load_article(id).await?;
        if dated.is_some_and(|article| format_date(article.published_at) == format_date(day)) {
            return Ok(());
        }
        blog.date_article(actor, id, day).await
    })
    .await
}
