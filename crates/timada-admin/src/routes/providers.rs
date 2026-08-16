use std::collections::BTreeMap;
use std::sync::Arc;

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use timada::provider_connection::ProviderConnectionState;
use timada::read_model::catalog_detail;
use timada::read_model::provider_list::{self, ProviderListRow};
use timada_provider::{Provider, ProviderContext, ProviderError};

use crate::AdminContext;
use crate::render::{AdminError, html};

/// Everything the providers templates show about one connection.
struct ConnectionView {
    id: String,
    provider_kind: String,
    display_name: String,
    enabled: bool,
    credentials_configured: bool,
}

impl From<ProviderListRow> for ConnectionView {
    fn from(row: ProviderListRow) -> Self {
        Self {
            id: row.id,
            provider_kind: row.provider_kind,
            display_name: row.display_name,
            enabled: row.enabled,
            credentials_configured: row.credentials_configured,
        }
    }
}

impl From<&ProviderConnectionState> for ConnectionView {
    fn from(state: &ProviderConnectionState) -> Self {
        Self {
            id: state.id.clone(),
            provider_kind: state.provider_kind.clone(),
            display_name: state.display_name.clone(),
            enabled: state.enabled,
            credentials_configured: !state.config.is_empty(),
        }
    }
}

struct KindOption {
    kind: &'static str,
    name: &'static str,
}

#[derive(Template)]
#[template(path = "providers/index.html")]
struct IndexPage {
    base_path: String,
    kinds: Vec<KindOption>,
    connections: Vec<ConnectionView>,
}

pub(crate) async fn index(State(ctx): State<AdminContext>) -> Result<Response, AdminError> {
    let connections = provider_list::all(&ctx.read_db)
        .await?
        .into_iter()
        .map(ConnectionView::from)
        .collect();

    let mut kinds: Vec<KindOption> = ctx
        .providers
        .iter()
        .map(|p| KindOption {
            kind: p.kind(),
            name: p.display_name(),
        })
        .collect();
    kinds.sort_by_key(|k| k.kind);

    html(&IndexPage {
        base_path: ctx.base_path.clone(),
        kinds,
        connections,
    })
}

#[derive(serde::Deserialize)]
pub(crate) struct ConnectForm {
    provider_kind: String,
}

pub(crate) async fn connect(
    State(ctx): State<AdminContext>,
    Form(form): Form<ConnectForm>,
) -> Result<Response, AdminError> {
    let id = timada::provider_connection::connect_provider(
        &ctx.evento,
        &ctx.providers,
        &form.provider_kind,
    )
    .await?;

    Ok(Redirect::to(&format!("{}/providers/{id}", ctx.base_path)).into_response())
}

/// One credentials-form field, prefilled for non-secret values.
struct FieldView {
    key: &'static str,
    label: &'static str,
    secret: bool,
    value: String,
}

#[derive(Template)]
#[template(path = "providers/show.html")]
struct ShowPage {
    base_path: String,
    connection: ConnectionView,
    fields: Vec<FieldView>,
}

#[derive(Template)]
#[template(path = "providers/show.html", block = "status")]
struct StatusFragment {
    base_path: String,
    connection: ConnectionView,
}

pub(crate) async fn show(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    // Deliberate exception to "UI reads projections only": credentials never
    // reach a read-model table, so this page needs the write-side state for
    // the form's non-secret values — and deriving the whole view from it also
    // gives exact read-your-own-write right after "Connect", before the
    // provider_list projection catches up. Secret values are never rendered.
    let state = timada::provider_connection::load(&ctx.evento, &id)
        .await?
        .ok_or(AdminError::NotFound)?;

    let schema = ctx
        .providers
        .get(&state.provider_kind)
        .map(|p| p.config_schema())
        .unwrap_or_default();

    let fields = schema
        .into_iter()
        .map(|field| {
            let value = if field.secret {
                String::new()
            } else {
                state
                    .config
                    .iter()
                    .find(|(key, _)| key == field.key)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default()
            };
            FieldView {
                key: field.key,
                label: field.label,
                secret: field.secret,
                value,
            }
        })
        .collect();

    html(&ShowPage {
        base_path: ctx.base_path.clone(),
        connection: (&state).into(),
        fields,
    })
}

pub(crate) async fn save_credentials(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
    Form(input): Form<BTreeMap<String, String>>,
) -> Result<Response, AdminError> {
    let state = timada::provider_connection::load(&ctx.evento, &id)
        .await?
        .ok_or(AdminError::NotFound)?;

    let schema = ctx
        .providers
        .get(&state.provider_kind)
        .map(|p| p.config_schema())
        .unwrap_or_default();

    let mut config = Vec::new();
    for field in schema {
        let submitted = input.get(field.key).map(|v| v.trim()).unwrap_or_default();
        if !submitted.is_empty() {
            config.push((field.key.to_owned(), submitted.to_owned()));
        } else if field.secret {
            // A blank secret field means "keep the stored value".
            if let Some((key, value)) = state.config.iter().find(|(key, _)| key == field.key) {
                config.push((key.clone(), value.clone()));
            }
        }
        // A blank non-secret field clears that value.
    }

    timada::provider_connection::configure_provider_credentials(&ctx.evento, &id, config).await?;

    Ok(Redirect::to(&format!("{}/providers/{id}", ctx.base_path)).into_response())
}

pub(crate) async fn enable(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    timada::provider_connection::enable_provider(&ctx.evento, &ctx.providers, &id).await?;
    status_fragment(&ctx, &id).await
}

pub(crate) async fn disable(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    timada::provider_connection::disable_provider(&ctx.evento, &id).await?;
    status_fragment(&ctx, &id).await
}

/// TwinSpark fragment for the status block, rendered from the write-side
/// state so it reflects the command that just committed — the provider_list
/// projection catches up asynchronously.
async fn status_fragment(ctx: &AdminContext, id: &str) -> Result<Response, AdminError> {
    let state = timada::provider_connection::load(&ctx.evento, id)
        .await?
        .ok_or(AdminError::NotFound)?;

    html(&StatusFragment {
        base_path: ctx.base_path.clone(),
        connection: (&state).into(),
    })
}

fn provider_context(state: &ProviderConnectionState) -> ProviderContext {
    ProviderContext {
        connection_id: state.id.clone(),
        config: state.config.iter().cloned().collect(),
    }
}

fn registered_provider(ctx: &AdminContext, kind: &str) -> Result<Arc<dyn Provider>, AdminError> {
    ctx.providers.get(kind).ok_or_else(|| {
        AdminError::Invalid(format!(
            "provider kind {kind:?} is not registered in this app"
        ))
    })
}

fn provider_error(error: ProviderError) -> AdminError {
    match error {
        ProviderError::NotFound(reference) => {
            AdminError::Invalid(format!("not found at provider: {reference}"))
        }
        ProviderError::Auth => AdminError::Invalid(
            "authentication with the provider failed — check credentials".to_owned(),
        ),
        ProviderError::RateLimited => {
            AdminError::Invalid("rate limited by the provider — try again later".to_owned())
        }
        ProviderError::Unsupported => {
            AdminError::Invalid("this provider does not support that operation".to_owned())
        }
        ProviderError::Other(error) => AdminError::Internal(error),
    }
}

/// One provider search result on the import page.
struct ImportItemView {
    external_ref: String,
    title: String,
    thumbnail_url: String,
    price: String,
    currency: String,
    /// The already-imported product for this reference, if any.
    imported_product_id: Option<String>,
}

impl ImportItemView {
    fn new(source: &timada_provider::SourceProduct, imported_product_id: Option<String>) -> Self {
        Self {
            external_ref: source.external_ref.clone(),
            title: source.title.clone(),
            thumbnail_url: source.image_urls.first().cloned().unwrap_or_default(),
            price: super::catalog::format_amount(source.price.amount_minor),
            currency: source.price.currency.clone(),
            imported_product_id,
        }
    }
}

#[derive(Template)]
#[template(path = "providers/import.html")]
struct ImportPage {
    base_path: String,
    connection: ConnectionView,
    q: String,
    error: Option<String>,
    items: Vec<ImportItemView>,
}

#[derive(Template)]
#[template(path = "providers/import_item.html")]
struct ImportItemFragment {
    base_path: String,
    connection: ConnectionView,
    item: ImportItemView,
}

#[derive(serde::Deserialize)]
pub(crate) struct ImportPageQuery {
    #[serde(default)]
    q: String,
}

pub(crate) async fn import_page(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
    Query(query): Query<ImportPageQuery>,
) -> Result<Response, AdminError> {
    let state = timada::provider_connection::load(&ctx.evento, &id)
        .await?
        .ok_or(AdminError::NotFound)?;

    let mut error = None;
    let mut items = Vec::new();
    if state.enabled {
        let provider = registered_provider(&ctx, &state.provider_kind)?;
        match provider
            .search_products(&provider_context(&state), &query.q, None)
            .await
        {
            Ok(page) => {
                for source in &page.items {
                    let imported = catalog_detail::find_by_source(
                        &ctx.read_db,
                        &state.id,
                        &source.external_ref,
                    )
                    .await?;
                    items.push(ImportItemView::new(source, imported));
                }
            }
            Err(provider_failure) => match provider_error(provider_failure) {
                AdminError::Invalid(message) => error = Some(message),
                other => return Err(other),
            },
        }
    }

    html(&ImportPage {
        base_path: ctx.base_path.clone(),
        connection: (&state).into(),
        q: query.q,
        error,
        items,
    })
}

#[derive(serde::Deserialize)]
pub(crate) struct ImportForm {
    external_ref: String,
}

pub(crate) async fn import(
    State(ctx): State<AdminContext>,
    Path(id): Path<String>,
    Form(form): Form<ImportForm>,
) -> Result<Response, AdminError> {
    let state = timada::provider_connection::load(&ctx.evento, &id)
        .await?
        .ok_or(AdminError::NotFound)?;
    if !state.enabled {
        return Err(AdminError::Invalid(
            "enable the connection before importing".to_owned(),
        ));
    }
    let provider = registered_provider(&ctx, &state.provider_kind)?;

    let source = provider
        .fetch_product(&provider_context(&state), &form.external_ref)
        .await
        .map_err(provider_error)?;

    // Best-effort dedup against the projection: a re-import racing the
    // subscription can still create a duplicate — an annoyance the admin can
    // archive, not corruption — so this stays a UI guard, not an invariant.
    let imported =
        match catalog_detail::find_by_source(&ctx.read_db, &state.id, &form.external_ref).await? {
            Some(existing) => existing,
            None => {
                timada::product::import_product(
                    &ctx.evento,
                    source.clone(),
                    &state.provider_kind,
                    &state.id,
                )
                .await?
            }
        };

    html(&ImportItemFragment {
        base_path: ctx.base_path.clone(),
        connection: (&state).into(),
        item: ImportItemView::new(&source, Some(imported)),
    })
}
