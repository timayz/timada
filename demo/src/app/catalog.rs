//! `/` and `/p/{product_id}`: the catalogue and the product page, with its
//! customer reviews, its questions & answers, and the forms to add to both.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::{
    brand_by_slug, category_lineage, category_tree, is_on_storefront, list_categories,
    load_product_page,
};
use timada_customer::customers_by_ids;
use timada_inventory::{
    InventoryError, RequestBackInStockAlert, StockLocation, alert_id, stock_item_id,
};
use timada_order::{OrderStatus, load_order_details, orders_of_customer};
use timada_pricing::{load_product_price, price_id};
use timada_review::{
    AskQuestion, ReviewError, ReviewStatus, SubmitReview, answers_of_questions,
    count_published_questions, load_review_details, own_unpublished_questions, product_rating,
    published_questions, published_reviews, review_id,
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
    Crumb, Head, account, breadcrumb, cart, category, document,
    format::{date, money},
    listing::{Listing, Scope, listing_view, load_listing},
    seo::{ProductOffer, breadcrumb_json_ld, json_ld_graph, product_json_ld},
};
use crate::{
    Store,
    auth::{current_account, require_account},
};

/// The top of the tree: the ways into the shop.
async fn departments(cx: &Cx) -> Result<Vec<(String, String)>> {
    let store = app_context::<Store>(cx);
    Ok(
        category_tree(list_categories(&store.db, false).await?, false)
            .into_iter()
            .map(|node| {
                let link =
                    href!(category::show, category::CategorySlug(node.category.slug)).resolve(cx);
                (link, node.category.name)
            })
            .collect(),
    )
}

fn listing_head(listing: &Listing, description: Option<String>) -> Head {
    Head {
        description,
        canonical: listing.canonical(),
        robots: listing.robots(),
        previous: listing.previous(),
        next: listing.next(),
        json_ld: None,
    }
}

#[page("/")]
pub async fn home(cx: &Cx) -> Result<impl View> {
    let departments = departments(cx).await?;
    let listing = load_listing(cx, href!(home).resolve(cx), Scope::default()).await?;
    let head = listing_head(
        &listing,
        Some("Le catalogue de la boutique de démonstration Timada.".to_owned()),
    );
    let seeded = listing.found.total > 0 || listing.filters.is_narrowed();
    Ok(view! {
        document(
            title: "Catalogue",
            head: Some(&head),
            <h1>"Catalogue"</h1>
            if !departments.is_empty() {
                <nav aria-label="Catégories">
                    <ul class="tags">
                        for (link, name) in &departments {
                            <li><a href=(link.clone())>(name.clone())</a></li>
                        }
                    </ul>
                </nav>
            }
            if seeded {
                listing_view(listing: &listing)
            } else {
                <p class="muted">"Aucun produit. Lancez " <code>"cargo run -p demo -- --seed"</code> "."</p>
            }
        )
    })
}

/// `/recherche?q=…`: the whole shop, searched. Never indexed: a search
/// always narrows.
#[page("/recherche")]
pub async fn search(cx: &Cx) -> Result<impl View> {
    let listing = load_listing(cx, href!(search).resolve(cx), Scope::default()).await?;
    let head = Head {
        robots: Some("noindex,follow"),
        ..listing_head(&listing, None)
    };
    let title = match &listing.filters.q {
        Some(q) => format!("Recherche : {q}"),
        None => "Recherche".to_owned(),
    };
    Ok(view! {
        document(
            title: &title,
            head: Some(&head),
            <h1>(title.clone())</h1>
            listing_view(listing: &listing)
        )
    })
}

path_param!(pub brand_slug: String, error = not_found);

/// `/marque/{brand_slug}`: what a brand sells here. A brand with nothing on
/// sale has no page.
#[page("/marque/{brand_slug}")]
pub async fn brand(cx: &Cx) -> Result<impl View> {
    let slug = param::<BrandSlug>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let name = brand_by_slug(&store.db, &slug).await?.ok_or_not_found()?;
    let here = href!(brand, BrandSlug(slug.clone())).resolve(cx);
    let listing = load_listing(
        cx,
        here.clone(),
        Scope {
            brand_slug: Some(slug),
            ..Scope::default()
        },
    )
    .await?;
    let trail = vec![
        Crumb {
            label: "Catalogue".to_owned(),
            link: Some(href!(home).resolve(cx)),
        },
        Crumb {
            label: name.clone(),
            link: None,
        },
    ];
    let head = Head {
        json_ld: Some(breadcrumb_json_ld(&trail, &here)),
        ..listing_head(
            &listing,
            Some(format!("Les produits {name} de la boutique.")),
        )
    };
    Ok(view! {
        document(
            title: &name,
            head: Some(&head),
            breadcrumb(trail: &trail)
            <h1>(name.clone())</h1>
            listing_view(listing: &listing)
        )
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
/// everyone once moderation let it through.
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

path_param!(pub question_id: String, error = not_found);

#[derive(Debug, Deserialize)]
pub struct AnswerForm {
    body: String,
}

/// A signed-in shopper answers a published question; the answer waits for
/// moderation. What the review context refuses comes back as a message.
#[page(POST "/p/{product_id}/questions/{question_id}/answers")]
pub async fn submit_answer(cx: &Cx, Form(form): Form<AnswerForm>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let id = param::<ProductId>(cx)?.clone();
    let question = param::<QuestionId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let reviews = timada_review::Command(&store.executor);
    // The question must be one of this product's.
    reviews
        .load_question(&question)
        .await?
        .filter(|q| q.product_id == id)
        .ok_or_not_found()?;

    let error = match reviews
        .submit_answer(&question, &account.customer_id, form.body)
        .await
    {
        Ok(_) => {
            let back = format!(
                "{}#questions",
                href!(product_page, ProductId(id)).resolve(cx)
            );
            return Err(see_other(back).into());
        }
        Err(ReviewError::Required(_)) => "Écrivez votre réponse avant de l'envoyer.",
        Err(ReviewError::AlreadyAnswered) => "Vous avez déjà répondu à cette question.",
        Err(ReviewError::QuestionNotPublished) => "Cette question n'est pas ouverte aux réponses.",
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Ok(view! { product_view(review_error: None, question_error: Some(error.to_owned())) })
}

/// One published question with its answers, ready to render.
struct QuestionLine {
    body: String,
    asked: String,
    /// `(author, text, date)` of the published answers.
    answers: Vec<(String, String, String)>,
    /// Where the signed-in shopper's own answer stands, if they gave one.
    own_answer: Option<&'static str>,
    answer_action: String,
    answer_field: String,
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
    // What the product costs in the shopper's currency; `None` when it is
    // not sold in it (or not sold any more).
    let currency = crate::currency::shopper_currency(cx).await?;
    let price = load_product_price(&store.executor, price_id(&id))
        .await?
        .and_then(|p| p.price_in(&currency));
    let not_in_currency = format!(
        "Ce produit n'est pas vendu en {}.",
        timada_core::format::currency_symbol(&currency)
    );
    // The way back up: the product's category, while the shop shows it; the
    // label the product was created with otherwise.
    let lineage = match &product.category_id {
        Some(category_id) => category_lineage(&store.db, category_id).await?,
        None => Vec::new(),
    };
    let trail: Vec<Crumb> = if is_on_storefront(&lineage) {
        category::crumbs(cx, &lineage, true)
    } else {
        Vec::new()
    };
    // The technical sheet, its lines gathered by group in the order given.
    let mut sheet: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for spec in &product.specs {
        let line = (spec.label.clone(), spec.value.clone());
        match sheet.iter_mut().find(|(group, _)| *group == spec.group) {
            Some((_, lines)) => lines.push(line),
            None => sheet.push((spec.group.clone(), vec![line])),
        }
    }
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
    // For search engines: the way down to the product, and what is on offer.
    let here = href!(product_page, ProductId(id.clone())).resolve(cx);
    let mut described = Vec::new();
    if !trail.is_empty() {
        let mut steps: Vec<Crumb> = trail
            .iter()
            .map(|crumb| Crumb {
                label: crumb.label.clone(),
                link: crumb.link.clone(),
            })
            .collect();
        steps.push(Crumb {
            label: product.name.clone(),
            link: None,
        });
        described.push(breadcrumb_json_ld(&steps, &here));
    }
    described.push(product_json_ld(
        &ProductOffer {
            name: &product.name,
            sku: &product.sku,
            brand: &product.brand.name,
            description: &product.short_description,
            image: product
                .media
                .iter()
                .find(|media| media.kind == timada_catalog::MediaKind::Image)
                .map(|media| media.url.as_str()),
            price: price.as_ref().map(|price| {
                (
                    price.price_incl_tax.minor,
                    price.price_incl_tax.currency.as_str(),
                )
            }),
            in_stock: available > 0,
            rating: rating
                .average_rating
                .map(|average| (average, rating.review_count)),
        },
        &here,
    ));
    let head = Head {
        description: Some(product.short_description.clone()).filter(|text| !text.is_empty()),
        // Review and question pages are the same product.
        canonical: Some(here.clone()),
        json_ld: Some(json_ld_graph(&described)),
        ..Head::default()
    };
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
        count_published_questions(&store.db, &id).await?,
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

    let asked = published_questions(
        &store.db,
        &id,
        QUESTIONS_PER_PAGE,
        (questions_pager.current - 1) * QUESTIONS_PER_PAGE,
    )
    .await?;
    let question_ids: Vec<String> = asked.iter().map(|q| q.question_id.clone()).collect();
    let me = account.as_ref().map(|a| a.customer_id.clone());
    let mut answers_by_question: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    // The shopper's own answers, whatever moderation made of them.
    let mut own_answers: HashMap<String, &'static str> = HashMap::new();
    for answer in answers_of_questions(&store.db, &question_ids, false).await? {
        if me.is_some() && answer.author_customer_id == me {
            let standing = match answer.status.as_str() {
                "published" => "Vous avez répondu à cette question.",
                "rejected" => "Votre réponse n'a pas été retenue.",
                _ => "Merci ! Votre réponse sera visible une fois validée par notre équipe.",
            };
            own_answers.insert(answer.question_id.clone(), standing);
        }
        if answer.status != "published" {
            continue;
        }
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
            own_answer: own_answers.get(&q.question_id).copied(),
            answer_action: href!(
                submit_answer,
                ProductId(id.clone()),
                QuestionId(q.question_id.clone())
            )
            .resolve(cx),
            answer_field: format!("answer-{}", q.question_id),
            body: q.body,
            asked: date(q.asked_at as u64),
        })
        .collect();
    // The shopper's own questions that are not public: `(text, standing)`.
    let own_questions: Vec<(String, String)> = match &account {
        Some(account) => own_unpublished_questions(&store.db, &id, &account.customer_id)
            .await?
            .into_iter()
            .map(|q| {
                let standing = match q.rejection_reason {
                    Some(reason) => format!("non retenue : {reason}"),
                    None => "en attente de validation".to_owned(),
                };
                (q.body, standing)
            })
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
            head: Some(&head),
            if trail.is_empty() {
                <p class="muted">(product.category_path.join(" > "))</p>
            } else {
                breadcrumb(trail: &trail)
            }
            <h1>(product.name.clone())</h1>
            if let Some(summary) = &rating_summary {
                <p><a href="#avis">(summary.clone())</a></p>
            }
            <p>(product.short_description.clone())</p>
            match &price {
                Some(price) => {
                    <p class="price">(money(&price.price_incl_tax))</p>
                    if let Some((offer, amount)) = &price.installment {
                        <p class="muted">"ou " (offer.count.to_string()) " × " (money(amount))</p>
                    }
                }
                None => <p class="muted">(not_in_currency.clone())</p>,
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
            if !sheet.is_empty() {
                <h2 id="fiche-technique">"Fiche technique"</h2>
                <table class="sheet">
                    for (group, lines) in &sheet {
                        <tbody>
                            if !group.is_empty() {
                                <tr><th colspan="2" scope="colgroup">(group.clone())</th></tr>
                            }
                            for (name, value) in lines {
                                <tr><th scope="row">(name.clone())</th><td>(value.clone())</td></tr>
                            }
                        </tbody>
                    }
                </table>
            }

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
                    if question.answers.is_empty() {
                        <p class="muted">"Pas encore de réponse."</p>
                    }
                    for (author, text, answered) in &question.answers {
                        <p><strong>(author.clone())</strong> " — " (text.clone()) <span class="muted">" (" (answered.clone()) ")"</span></p>
                    }
                    match question.own_answer {
                        Some(standing) => { <p role="status" class="notice">(standing)</p> }
                        None => {
                            if signed_in {
                                <form method="post" action=(question.answer_action.clone())>
                                    <p>
                                        <label for=(question.answer_field.clone())>"Votre réponse"</label>
                                        <br>
                                        <textarea id=(question.answer_field.clone()) name="body" rows="2" cols="60" maxlength="1000" required=(true)></textarea>
                                    </p>
                                    <button type="submit">"Répondre"</button>
                                </form>
                            }
                        }
                    }
                </article>
            }
            page_links(pager: &questions_pager, label: "Questions")
            if !own_questions.is_empty() {
                <p role="status" class="notice">"Vos questions qui ne sont pas publiées :"</p>
                <ul>for (body, standing) in &own_questions { <li>(body.clone()) <span class="muted">" — " (standing.clone())</span></li> }</ul>
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
