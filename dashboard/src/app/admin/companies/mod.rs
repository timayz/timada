//! `/admin/companies`: every company on the platform, for administrators,
//! who release a SIREN registered by someone it does not belong to.

pub mod release;

use timada_tenancy::all_companies;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use crate::app::admin::require_admin;
use crate::app::auth::Flash;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{Shell, document, empty_row, format_time, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct CompaniesQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn admin_companies_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(index).query(Flash { error, message }).resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    require_admin(cx).await?;
    let query = query_params::<CompaniesQuery>(cx)?;
    let rows = all_companies(app_context::<Db>(cx), 0, 200)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(view! {
        document(
            title: "Companies · Administration",
            shell: Shell::page("admin_companies"),
            page_header(title: "Companies", subtitle: Some("Every company registered on the platform."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            section_card(
                description: Some("A SIREN belongs to one company. When someone registered a SIREN that is not theirs, release it: its rightful company can then register it."),
                table(
                    table_header(
                        table_row(
                            table_head("Company")
                            table_head("SIREN")
                            table_head("Registered")
                            table_head(<span class="sr-only">"Actions"</span>)
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 4, text: "No company yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-admin-company=(&row.siren) },
                                table_cell(<span class="font-medium">(&row.legal_name)</span>)
                                table_cell(<code class="text-xs">(&row.siren)</code>)
                                table_cell(<time datetime=(row.created_at)>(format_time(row.created_at as u64))</time>)
                                table_cell(
                                    attrs: attributes! { class="text-right" },
                                    <form method="post" action=(href!(release::release))>
                                        <input type="hidden" name="siren" value=(&row.siren)>
                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="text-destructive-text" }, "Release the SIREN")
                                    </form>
                                )
                            )
                        }
                    )
                )
            )
        )
    })
}
