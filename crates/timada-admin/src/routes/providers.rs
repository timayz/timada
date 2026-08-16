use std::collections::BTreeMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use timada::provider_connection::ProviderConnectionState;
use timada::read_model::provider_list::{self, ProviderListRow};

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
