use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use timada::product::ProductState;
use timada::read_model::catalog_list::{self, CatalogListRow};
use timada_provider::{Money, SourceProduct};

use crate::AdminContext;
use crate::render::{AdminError, html};

const PAGE_SIZE: u16 = 20;

/// Format minor units as a decimal, e.g. `1234` → `"12.34"`.
fn format_amount(amount_minor: i64) -> String {
    format!("{}.{:02}", amount_minor / 100, (amount_minor % 100).abs())
}

/// Parse a decimal price like `"12.34"` into minor units.
fn parse_amount(input: &str) -> Result<i64, AdminError> {
    let input = input.trim();
    let invalid = || AdminError::Invalid(format!("invalid price: {input:?} (expected e.g. 12.34)"));

    let (whole, fraction) = match input.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (input, ""),
    };
    if fraction.len() > 2 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let whole: i64 = whole.parse().map_err(|_| invalid())?;
    let cents: i64 = if fraction.is_empty() {
        0
    } else {
        let padded = format!("{fraction:0<2}");
        padded.parse().map_err(|_| invalid())?
    };
    whole
        .checked_mul(100)
        .and_then(|minor| minor.checked_add(cents))
        .ok_or_else(invalid)
}

struct RowView {
    id: String,
    title: String,
    thumbnail_url: String,
    price: String,
    currency: String,
    status: String,
    provider_kind: String,
    stock_available: i64,
}

impl From<CatalogListRow> for RowView {
    fn from(row: CatalogListRow) -> Self {
        Self {
            price: format_amount(row.amount_minor),
            id: row.id,
            title: row.title,
            thumbnail_url: row.thumbnail_url,
            currency: row.currency,
            status: row.status,
            provider_kind: row.provider_kind,
            stock_available: row.stock_available,
        }
    }
}

#[derive(Template)]
#[template(path = "catalog/index.html")]
struct IndexPage {
    base_path: String,
    q: String,
    rows: Vec<RowView>,
    next: Option<String>,
}

#[derive(serde::Deserialize)]
pub(crate) struct IndexQuery {
    #[serde(default)]
    q: String,
    after: Option<String>,
}

pub(crate) async fn index(
    State(ctx): State<AdminContext>,
    Query(query): Query<IndexQuery>,
) -> Result<Response, AdminError> {
    let page = catalog_list::page(
        &ctx.read_db,
        PAGE_SIZE,
        query.after,
        Some(&query.q),
        false,
    )
    .await?;

    let next = page
        .page_info
        .has_next_page
        .then(|| page.page_info.end_cursor.as_ref().map(|cursor| cursor.0.clone()))
        .flatten();
    let rows = page.edges.into_iter().map(|edge| edge.node.into()).collect();

    html(&IndexPage {
        base_path: ctx.base_path.clone(),
        q: query.q,
        rows,
        next,
    })
}

#[derive(Template)]
#[template(path = "catalog/new.html")]
struct NewPage {
    base_path: String,
}

pub(crate) async fn new(State(ctx): State<AdminContext>) -> Result<Response, AdminError> {
    html(&NewPage {
        base_path: ctx.base_path.clone(),
    })
}

#[derive(serde::Deserialize)]
pub(crate) struct NewProductForm {
    title: String,
    #[serde(default)]
    description: String,
    price: String,
    currency: String,
    #[serde(default)]
    image_url: String,
    #[serde(default)]
    initial_stock: String,
}

pub(crate) async fn create(
    State(ctx): State<AdminContext>,
    Form(form): Form<NewProductForm>,
) -> Result<Response, AdminError> {
    let amount_minor = parse_amount(&form.price)?;
    let image_url = form.image_url.trim();

    let source = SourceProduct {
        // For self-inventory products the provider-side reference is the
        // product id itself, which doesn't exist yet — left empty.
        external_ref: String::new(),
        title: form.title,
        description: form.description,
        image_urls: if image_url.is_empty() {
            Vec::new()
        } else {
            vec![image_url.to_owned()]
        },
        price: Money {
            amount_minor,
            currency: form.currency.trim().to_uppercase(),
        },
        variants: Vec::new(),
    };

    let id =
        timada::product::import_product(&ctx.evento, source, timada::self_inventory::KIND, "")
            .await?;

    let initial_stock = form.initial_stock.trim();
    if !initial_stock.is_empty() {
        let quantity: i64 = initial_stock
            .parse()
            .map_err(|_| AdminError::Invalid(format!("invalid stock: {initial_stock:?}")))?;
        if quantity > 0 {
            timada::inventory::adjust_stock(&ctx.evento, &id, quantity, "initial stock".to_owned())
                .await?;
        }
    }

    Ok(Redirect::to(&format!("{}/catalog/{id}", ctx.base_path)).into_response())
}

/// Everything the product edit page shows, derived from write-side state so
/// the page is exact right after a command commits (see the module note on
/// consistency in `routes/providers.rs`).
struct ProductView {
    id: String,
    title: String,
    description: String,
    price: String,
    currency: String,
    published: bool,
    archived: bool,
    provider_kind: String,
    self_inventory: bool,
}

impl From<&ProductState> for ProductView {
    fn from(state: &ProductState) -> Self {
        Self {
            id: state.id.clone(),
            title: state.title.clone(),
            description: state.description.clone(),
            price: format_amount(state.price_amount_minor),
            currency: state.currency.clone(),
            published: state.published,
            archived: state.archived,
            provider_kind: state.provider_kind.clone(),
            self_inventory: state.provider_kind == timada::self_inventory::KIND,
        }
    }
}

#[derive(Template)]
#[template(path = "catalog/show.html")]
struct ShowPage {
    base_path: String,
    product: ProductView,
    stock_available: i64,
}

#[derive(Template)]
#[template(path = "catalog/show.html", block = "details")]
struct DetailsFragment {
    base_path: String,
    product: ProductView,
}

#[derive(Template)]
#[template(path = "catalog/show.html", block = "price")]
struct PriceFragment {
    base_path: String,
    product: ProductView,
}

#[derive(Template)]
#[template(path = "catalog/show.html", block = "status")]
struct StatusFragment {
    base_path: String,
    product: ProductView,
}

#[derive(Template)]
#[template(path = "catalog/show.html", block = "stock")]
struct StockFragment {
    base_path: String,
    product: ProductView,
    stock_available: i64,
}

async fn load_product(ctx: &AdminContext, id: &str) -> Result<ProductState, AdminError> {
    timada::product::load(&ctx.evento, id)
        .await?
        .ok_or(AdminError::NotFound)
}

async fn load_stock(ctx: &AdminContext, id: &str) -> Result<i64, AdminError> {
    Ok(timada::inventory::load(&ctx.evento, id)
        .await?
        .map_or(0, |state| state.available))
}

pub(crate) async fn show(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    let state = load_product(&ctx, &id).await?;
    let stock_available = load_stock(&ctx, &id).await?;

    html(&ShowPage {
        base_path: ctx.base_path.clone(),
        product: (&state).into(),
        stock_available,
    })
}

#[derive(serde::Deserialize)]
pub(crate) struct DetailsForm {
    title: String,
    #[serde(default)]
    description: String,
}

pub(crate) async fn revise_details(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
    Form(form): Form<DetailsForm>,
) -> Result<Response, AdminError> {
    timada::product::revise_product_details(&ctx.evento, &id, form.title, form.description)
        .await?;

    let state = load_product(&ctx, &id).await?;
    html(&DetailsFragment {
        base_path: ctx.base_path.clone(),
        product: (&state).into(),
    })
}

#[derive(serde::Deserialize)]
pub(crate) struct PriceForm {
    price: String,
    currency: String,
}

pub(crate) async fn reprice(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
    Form(form): Form<PriceForm>,
) -> Result<Response, AdminError> {
    let amount_minor = parse_amount(&form.price)?;
    timada::product::reprice_product(
        &ctx.evento,
        &id,
        amount_minor,
        form.currency.trim().to_uppercase(),
    )
    .await?;

    let state = load_product(&ctx, &id).await?;
    html(&PriceFragment {
        base_path: ctx.base_path.clone(),
        product: (&state).into(),
    })
}

pub(crate) async fn publish(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    timada::product::publish_product(&ctx.evento, &id).await?;
    status_fragment(&ctx, &id).await
}

pub(crate) async fn unpublish(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    timada::product::unpublish_product(&ctx.evento, &id).await?;
    status_fragment(&ctx, &id).await
}

pub(crate) async fn archive(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    timada::product::archive_product(&ctx.evento, &id).await?;
    status_fragment(&ctx, &id).await
}

async fn status_fragment(ctx: &AdminContext, id: &str) -> Result<Response, AdminError> {
    let state = load_product(ctx, id).await?;
    html(&StatusFragment {
        base_path: ctx.base_path.clone(),
        product: (&state).into(),
    })
}

#[derive(serde::Deserialize)]
pub(crate) struct StockForm {
    quantity_change: String,
    #[serde(default)]
    reason: String,
}

pub(crate) async fn adjust_stock(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
    Form(form): Form<StockForm>,
) -> Result<Response, AdminError> {
    let quantity_change: i64 = form.quantity_change.trim().parse().map_err(|_| {
        AdminError::Invalid(format!(
            "invalid quantity: {:?} (expected e.g. 5 or -2)",
            form.quantity_change
        ))
    })?;
    let reason = if form.reason.trim().is_empty() {
        "manual adjustment".to_owned()
    } else {
        form.reason.trim().to_owned()
    };

    let stock_available =
        timada::inventory::adjust_stock(&ctx.evento, &id, quantity_change, reason).await?;

    let state = load_product(&ctx, &id).await?;
    html(&StockFragment {
        base_path: ctx.base_path.clone(),
        product: (&state).into(),
        stock_available,
    })
}
