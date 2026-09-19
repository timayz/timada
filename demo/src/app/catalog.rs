//! `/` and `/p/{product_id}`: the catalogue and the product page, with its
//! customer reviews, its questions & answers, and the forms to add to both.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::{ListProducts, ProductListRow, list_products, load_product_page};
use timada_customer::customers_by_ids;
use timada_inventory::{
    InventoryError, RequestBackInStockAlert, StockLocation, alert_id, stock_item_id,
};
use timada_order::{OrderStatus, load_order_details, orders_of_customer};
use timada_pricing::{load_product_price, price_id};
use timada_review::{
    AskQuestion, ReviewError, ReviewStatus, SubmitReview, answered_questions, answers_of_questions,
    count_answered_questions, load_review_details, product_rating, published_reviews, review_id,
    unanswered_questions_of,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form, error::RouterErrorExt, error::see_other, href, page, path_param,
        path_param as param, query_params, query_params as query,
    },
    view::{View, component, view},
};

use super::{
    account, cart, document,
    format::{date, money},
};
use crate::{
    Store,
    auth::{current_account, require_account},
};

#[page("/")]
pub async fn home(cx: &Cx) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    let products = list_products(&store.db, &ListProducts::default()).await?;
    Ok(view! {
        document(
            title: "Catalogue",
            <h1>"Catalogue"</h1>
            if products.is_empty() {
                <p class="muted">"Aucun produit. Lancez " <code>"cargo run -p demo -- --seed"</code> "."</p>
            } else {
                <ul>
                    for product in &products { product_item(product: product) }
                </ul>
            }
        )
    })
}

#[component]
async fn product_item(cx: &Cx, product: &ProductListRow) -> Result<impl View> {
    let link = href!(product_page, ProductId(product.id.clone())).resolve(cx);
    Ok(view! {
        <li><a href=(link)>(product.name.clone())</a> " " <span class="muted">(product.sku.clone())</span></li>
    })
}

path_param!(pub product_id: String, error = not_found);

/// Reviews and questions are paged separately: `?avis=2`, `?questions=3`.
const REVIEWS_PER_PAGE: u32 = 5;
const QUESTIONS_PER_PAGE: u32 = 5;

#[query_params(error = bad_request)]
struct ProductQuery {
    avis: Option<u32>,
    questions: Option<u32>,
}

/// "Plus récents" / "plus anciens" links of one of the two lists.
struct Pager {
    current: u32,
    pages: u32,
    previous: Option<String>,
    next: Option<String>,
}

/// The pager of the list paged by `param`; `other` is the other list's
/// parameter and page, which the links keep.
fn pager(
    base: &str,
    param: &str,
    other: (&str, u32),
    current: u32,
    total: i64,
    per_page: u32,
) -> Pager {
    let pages = (total.max(0) as u32).div_ceil(per_page).max(1);
    let current = current.clamp(1, pages);
    let link = |page: u32| {
        let mut query = vec![format!("{param}={page}")];
        if other.1 > 1 {
            query.push(format!("{}={}", other.0, other.1));
        }
        format!("{base}?{}#{param}", query.join("&"))
    };
    Pager {
        current,
        pages,
        previous: (current > 1).then(|| link(current - 1)),
        next: (current < pages).then(|| link(current + 1)),
    }
}

#[page("/p/{product_id}")]
pub async fn product_page() -> Result<impl View> {
    Ok(view! { product_view(review_error: None, question_error: None) })
}

#[derive(Debug, Deserialize)]
pub struct ReviewForm {
    rating: u8,
    title: String,
    body: String,
}

/// A signed-in shopper leaves a review; it waits for moderation.
#[page(POST "/p/{product_id}/reviews")]
pub async fn submit_review(cx: &Cx, Form(form): Form<ReviewForm>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<ProductId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    load_product_page(&store.executor, &id)
        .await?
        .ok_or_not_found()?;

    let order_id = purchase_of(store, &account.customer_id, &id).await?;
    let submitted = timada_review::Command(&store.executor)
        .submit_review(SubmitReview {
            product_id: id.clone(),
            customer_id: account.customer_id.clone(),
            order_id,
            rating: form.rating,
            title: form.title.trim().to_owned(),
            body: form.body.trim().to_owned(),
        })
        .await;
    let error = match submitted {
        Ok(_) => {
            let back = format!("{}#avis", href!(product_page, ProductId(id)).resolve(cx));
            return Err(see_other(back).into());
        }
        Err(ReviewError::InvalidRating(_)) => "Choisissez une note de 1 à 5.",
        Err(ReviewError::Required(_)) => "Écrivez votre avis avant de l'envoyer.",
        Err(ReviewError::AlreadyReviewed) => "Vous avez déjà donné votre avis sur ce produit.",
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Ok(view! { product_view(review_error: Some(error.to_owned()), question_error: None) })
}

/// "M'alerter du retour en stock": a signed-in shopper asks to be told when
/// an out-of-stock product is back. Asking twice, or for a product that is
/// in stock, changes nothing.
#[page(POST "/p/{product_id}/alert")]
pub async fn request_alert(cx: &Cx) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<ProductId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    load_product_page(&store.executor, &id)
        .await?
        .ok_or_not_found()?;

    if available_stock(store, &id).await? == 0 {
        let requested = timada_inventory::Command(&store.executor)
            .request_back_in_stock_alert(RequestBackInStockAlert {
                product_id: id.clone(),
                customer_id: account.customer_id.clone(),
                email: account.email.clone(),
            })
            .await;
        match requested {
            Ok(_) | Err(InventoryError::AlreadyRequested) => {}
            Err(err) => return Err(anyhow::Error::from(err).into()),
        }
    }
    Err::<(), _>(see_other(href!(product_page, ProductId(id)).resolve(cx)).into())
}

#[derive(Debug, Deserialize)]
pub struct QuestionForm {
    body: String,
}

/// A signed-in shopper asks about the product; the question shows to
/// everyone once it is answered.
#[page(POST "/p/{product_id}/questions")]
pub async fn ask_question(cx: &Cx, Form(form): Form<QuestionForm>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<ProductId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    load_product_page(&store.executor, &id)
        .await?
        .ok_or_not_found()?;

    let asked = timada_review::Command(&store.executor)
        .ask_question(AskQuestion {
            product_id: id.clone(),
            customer_id: account.customer_id.clone(),
            body: form.body.trim().to_owned(),
        })
        .await;
    let error = match asked {
        Ok(_) => {
            let back = format!(
                "{}#questions",
                href!(product_page, ProductId(id)).resolve(cx)
            );
            return Err(see_other(back).into());
        }
        Err(ReviewError::Required(_)) => "Écrivez votre question avant de l'envoyer.",
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Ok(view! { product_view(review_error: None, question_error: Some(error.to_owned())) })
}

/// The shopper's order that contains the product, if any: it makes the
/// review a verified purchase.
async fn purchase_of(
    store: &Store,
    customer_id: &str,
    product_id: &str,
) -> anyhow::Result<Option<String>> {
    for row in orders_of_customer(&store.db, customer_id).await? {
        if row.status == OrderStatus::Cancelled.as_str() {
            continue;
        }
        let bought = load_order_details(&store.executor, &row.order_id)
            .await?
            .is_some_and(|o| o.lines.iter().any(|l| l.product_id == product_id));
        if bought {
            return Ok(Some(row.order_id));
        }
    }
    Ok(None)
}

/// One published review, ready to render.
struct ReviewLine {
    author: String,
    stars: String,
    rating_label: String,
    title: String,
    body: String,
    written: String,
    verified: bool,
}

/// One answered question with its answers, ready to render.
struct QuestionLine {
    body: String,
    asked: String,
    /// `(author, text, date)`.
    answers: Vec<(String, String, String)>,
}

/// What the signed-in shopper may do in the reviews section.
enum ReviewAccess {
    SignIn(String),
    Write,
    Pending,
    Published,
    Rejected(String),
}

fn stars(rating: i64) -> String {
    let full = rating.clamp(0, 5) as usize;
    format!("{}{}", "★".repeat(full), "☆".repeat(5 - full))
}

#[component]
async fn product_view(
    cx: &Cx,
    review_error: Option<String>,
    question_error: Option<String>,
) -> Result<impl View> {
    let id = param::<ProductId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let product = load_product_page(&store.executor, &id)
        .await?
        .ok_or_not_found()?;
    let price = load_product_price(&store.executor, price_id(&id))
        .await?
        .filter(|p| !p.withdrawn);
    let available = available_stock(store, &id).await?;
    let availability = if available > 0 {
        format!("En stock ({available} disponibles)")
    } else {
        "Rupture".to_owned()
    };

    let rating = product_rating(&store.db, &id).await?;
    let rating_summary = rating.average_rating.map(|average| {
        format!(
            "{} / 5 — {} avis",
            format!("{average:.1}").replace('.', ","),
            rating.review_count
        )
    });
    let query = query::<ProductQuery>(cx)?;
    let (review_page, question_page) = (
        query.avis.unwrap_or(1).max(1),
        query.questions.unwrap_or(1).max(1),
    );
    let base = href!(product_page, ProductId(id.clone())).resolve(cx);
    let reviews_pager = pager(
        &base,
        "avis",
        ("questions", question_page),
        review_page,
        rating.review_count,
        REVIEWS_PER_PAGE,
    );
    let questions_pager = pager(
        &base,
        "questions",
        ("avis", review_page),
        question_page,
        count_answered_questions(&store.db, &id).await?,
        QUESTIONS_PER_PAGE,
    );
    let rows = published_reviews(
        &store.db,
        &id,
        REVIEWS_PER_PAGE,
        (reviews_pager.current - 1) * REVIEWS_PER_PAGE,
    )
    .await?;
    let author_ids: Vec<String> = rows.iter().map(|r| r.customer_id.clone()).collect();
    let authors: HashMap<String, String> = customers_by_ids(&store.db, &author_ids)
        .await?
        .into_iter()
        .map(|c| {
            let initial = c.last_name.chars().next().map(|i| format!(" {i}."));
            (
                c.customer_id,
                format!("{}{}", c.first_name, initial.unwrap_or_default()),
            )
        })
        .collect();
    let reviews: Vec<ReviewLine> = rows
        .into_iter()
        .map(|row| ReviewLine {
            author: authors
                .get(&row.customer_id)
                .cloned()
                .unwrap_or_else(|| "Client".to_owned()),
            stars: stars(row.rating),
            rating_label: format!("Note : {} sur 5", row.rating),
            title: row.title,
            body: row.body,
            written: date(row.submitted_at as u64),
            verified: row.verified_purchase,
        })
        .collect();

    let account = match current_account(cx).await {
        Ok(account) => account.clone(),
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };

    let asked = answered_questions(
        &store.db,
        &id,
        QUESTIONS_PER_PAGE,
        (questions_pager.current - 1) * QUESTIONS_PER_PAGE,
    )
    .await?;
    let question_ids: Vec<String> = asked.iter().map(|q| q.question_id.clone()).collect();
    let mut answers_by_question: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for answer in answers_of_questions(&store.db, &question_ids).await? {
        let author = match answer.author_customer_id {
            None => "Réponse de la boutique",
            Some(_) => "Réponse d'un client",
        };
        answers_by_question
            .entry(answer.question_id)
            .or_default()
            .push((
                author.to_owned(),
                answer.body,
                date(answer.answered_at as u64),
            ));
    }
    let questions: Vec<QuestionLine> = asked
        .into_iter()
        .map(|q| QuestionLine {
            answers: answers_by_question
                .remove(&q.question_id)
                .unwrap_or_default(),
            body: q.body,
            asked: date(q.asked_at as u64),
        })
        .collect();
    // The shopper's own questions still waiting for an answer.
    let own_questions: Vec<String> = match &account {
        Some(account) => unanswered_questions_of(&store.db, &id, &account.customer_id)
            .await?
            .into_iter()
            .map(|q| q.body)
            .collect(),
        None => Vec::new(),
    };
    // Out of stock: can this shopper ask for an alert, or is one pending?
    let alert_pending = match &account {
        Some(account) if available == 0 => timada_inventory::Command(&store.executor)
            .load_alert(alert_id(&id, &account.customer_id))
            .await?
            .is_some_and(|alert| alert.is_pending()),
        _ => false,
    };
    let alert_action = href!(request_alert, ProductId(id.clone())).resolve(cx);
    let signed_in = account.is_some();
    let login_link = href!(account::login)
        .query([(
            "next",
            href!(product_page, ProductId(id.clone())).resolve(cx),
        )])
        .resolve(cx);
    let question_action = href!(ask_question, ProductId(id.clone())).resolve(cx);
    let access = match account {
        None => ReviewAccess::SignIn(
            href!(account::login)
                .query([(
                    "next",
                    href!(product_page, ProductId(id.clone())).resolve(cx),
                )])
                .resolve(cx),
        ),
        Some(account) => {
            let own =
                load_review_details(&store.executor, review_id(&id, &account.customer_id)).await?;
            match own.map(|r| (r.status, r.rejection_reason)) {
                None => ReviewAccess::Write,
                Some((ReviewStatus::Pending, _)) => ReviewAccess::Pending,
                Some((ReviewStatus::Published, _)) => ReviewAccess::Published,
                Some((ReviewStatus::Rejected, reason)) => {
                    ReviewAccess::Rejected(reason.unwrap_or_default())
                }
            }
        }
    };
    let review_action = href!(submit_review, ProductId(id.clone())).resolve(cx);

    Ok(view! {
        document(
            title: &product.name,
            <p class="muted">(product.category_path.join(" > "))</p>
            <h1>(product.name.clone())</h1>
            if let Some(summary) = &rating_summary {
                <p><a href="#avis">(summary.clone())</a></p>
            }
            <p>(product.short_description.clone())</p>
            match &price {
                Some(price) => {
                    <p class="price">(money(&price.price_incl_tax))</p>
                    if let Some(amount) = &price.installment_amount {
                        <p class="muted">"ou 3 × " (money(amount))</p>
                    }
                }
                None => <p class="muted">"Prix indisponible"</p>,
            }
            <p>(availability) " · garantie " (product.warranty_months.to_string()) " mois"</p>
            if price.is_some() && available > 0 && !product.archived {
                <form method="post" action=(href!(cart::add))>
                    <input type="hidden" name="product_id" value=(id.clone())>
                    <label for="quantity">"Quantité"</label>
                    " "
                    <input id="quantity" name="quantity" type="number" min="1" max=(available.to_string()) value="1" required=(true)>
                    " "
                    <button type="submit">"Ajouter au panier"</button>
                </form>
            }
            if available == 0 && !product.archived {
                if alert_pending {
                    <p role="status" class="notice">"Alerte enregistrée : vous serez prévenu du retour en stock dans " <a href=(href!(account::alerts))>"vos alertes"</a> "."</p>
                } else if signed_in {
                    <form method="post" action=(alert_action.clone())>
                        <button type="submit">"M'alerter du retour en stock"</button>
                    </form>
                } else {
                    <p><a href=(login_link.clone())>"Connectez-vous"</a> " pour être alerté du retour en stock."</p>
                }
            }
            if !product.key_features.is_empty() {
                <h2>"Caractéristiques principales"</h2>
                <ul>for feature in &product.key_features { <li>(feature.clone())</li> }</ul>
            }
            if !product.long_description.is_empty() { <p>(product.long_description.clone())</p> }

            <h2 id="avis">"Avis clients"</h2>
            if reviews.is_empty() {
                <p class="muted">"Aucun avis pour le moment."</p>
            }
            for review in &reviews {
                <article class="card">
                    <h3>
                        <span aria-hidden="true">(review.stars.clone())</span>
                        <span class="muted">" " (review.rating_label.clone())</span>
                        if !review.title.is_empty() { " — " (review.title.clone()) }
                    </h3>
                    <p>(review.body.clone())</p>
                    <p class="muted">
                        (review.author.clone()) ", le " (review.written.clone())
                        if review.verified { " · Achat vérifié" }
                    </p>
                </article>
            }
            if let Some(error) = &review_error { <p role="alert" class="error">(error.clone())</p> }
            page_links(pager: &reviews_pager, label: "Avis")
            match &access {
                ReviewAccess::SignIn(login) => {
                    <p><a href=(login.clone())>"Connectez-vous"</a> " pour donner votre avis."</p>
                }
                ReviewAccess::Write => {
                    <h3>"Donner mon avis"</h3>
                    <form method="post" action=(review_action.clone())>
                        <p>
                            <label for="rating">"Note"</label>
                            " "
                            <select id="rating" name="rating" required=(true)>
                                for (value, label) in [("5", "5 — Excellent"), ("4", "4 — Bien"), ("3", "3 — Correct"), ("2", "2 — Décevant"), ("1", "1 — Mauvais")] {
                                    <option value=(value)>(label)</option>
                                }
                            </select>
                        </p>
                        <p>
                            <label for="review-title">"Titre"</label>
                            " "
                            <input id="review-title" name="title" maxlength="120" autocomplete="off">
                        </p>
                        <p>
                            <label for="review-body">"Votre avis"</label>
                            <br>
                            <textarea id="review-body" name="body" rows="4" cols="60" maxlength="2000" required=(true)></textarea>
                        </p>
                        <button type="submit">"Envoyer mon avis"</button>
                    </form>
                }
                ReviewAccess::Pending => {
                    <p role="status" class="notice">"Merci ! Votre avis sera visible une fois validé par notre équipe."</p>
                }
                ReviewAccess::Published => {
                    <p class="muted">"Vous avez déjà donné votre avis sur ce produit."</p>
                }
                ReviewAccess::Rejected(reason) => {
                    <p role="status" class="notice">"Votre avis n'a pas été retenu : " (reason.clone())</p>
                }
            }

            <h2 id="questions">"Questions & réponses"</h2>
            if questions.is_empty() {
                <p class="muted">"Aucune question pour le moment."</p>
            }
            for question in &questions {
                <article class="card">
                    <h3>(question.body.clone())</h3>
                    <p class="muted">"Posée le " (question.asked.clone())</p>
                    for (author, text, answered) in &question.answers {
                        <p><strong>(author.clone())</strong> " — " (text.clone()) <span class="muted">" (" (answered.clone()) ")"</span></p>
                    }
                </article>
            }
            page_links(pager: &questions_pager, label: "Questions")
            if !own_questions.is_empty() {
                <p role="status" class="notice">"Vos questions en attente de réponse :"</p>
                <ul>for body in &own_questions { <li>(body.clone())</li> }</ul>
            }
            if let Some(error) = &question_error { <p role="alert" class="error">(error.clone())</p> }
            if signed_in {
                <form method="post" action=(question_action.clone())>
                    <p>
                        <label for="question-body">"Votre question sur ce produit"</label>
                        <br>
                        <textarea id="question-body" name="body" rows="3" cols="60" maxlength="1000" required=(true)></textarea>
                    </p>
                    <button type="submit">"Poser ma question"</button>
                </form>
            } else {
                <p><a href=(login_link.clone())>"Connectez-vous"</a> " pour poser une question."</p>
            }
        )
    })
}

#[component]
async fn page_links(pager: &Pager, label: &str) -> Result<impl View> {
    let position = format!("{label} — page {} sur {}", pager.current, pager.pages);
    Ok(view! {
        if pager.pages > 1 {
            <nav aria-label=(position.clone())>
                <p class="muted">
                    if let Some(link) = &pager.previous { <a href=(link.clone()) rel="prev">"← Plus récents"</a> " · " }
                    (position.clone())
                    if let Some(link) = &pager.next { " · " <a href=(link.clone()) rel="next">"Plus anciens →"</a> }
                </p>
            </nav>
        }
    })
}

/// Units the warehouse can still promise for a product.
pub async fn available_stock(store: &Store, product_id: &str) -> anyhow::Result<u32> {
    let stock = timada_inventory::load_stock_availability(
        &store.executor,
        stock_item_id(product_id, &StockLocation::Warehouse),
    )
    .await?;
    Ok(stock.map_or(0, |s| s.available))
}
