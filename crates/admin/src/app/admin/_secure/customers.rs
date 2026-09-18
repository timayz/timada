//! `/{mount}/customers`: registered customers with free-text search.

pub mod customer_id;

use timada_customer::{CustomerListRow, ListCustomers, count_customers, list_customers};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, view},
};

use crate::{
    components::{
        input::input,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date, empty_state, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct CustomersQuery {
    page: Option<u32>,
    q: Option<String>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<CustomersQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let db = &app_context::<AdminServices>(cx).db;
    let rows = list_customers(
        db,
        &ListCustomers {
            q: query.q.clone(),
            limit: PAGE_SIZE,
            offset: (page - 1) * PAGE_SIZE,
        },
    )
    .await?;
    let total = count_customers(db, query.q.as_deref()).await?;

    Ok(view! {
        page_header(
            title: "Clients",
            <form method="get" class="flex items-center gap-2 text-sm">
                input(attrs: topcoat::view::attributes! { type="search" name="q" placeholder="Email ou nom" value=(query.q.clone().unwrap_or_default()) })
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Rechercher"</button>
            </form>
        )
        if rows.is_empty() {
            empty_state(message: "Aucun client.")
        } else {
            table(
                table_header(table_row(
                    table_head("Inscrit le") table_head("Email") table_head("Nom") table_head("N° client")
                ))
                table_body(
                    for row in &rows { customer_row(row: row) }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

#[topcoat::view::component]
async fn customer_row(cx: &Cx, row: &CustomerListRow) -> Result<impl View> {
    let link = href!(
        customer_id::show,
        customer_id::CustomerId(row.customer_id.clone())
    )
    .resolve(cx);
    Ok(view! {
        table_row(
            table_cell((date(row.registered_at as u64)))
            table_cell(<a href=(link) class="underline-offset-4 hover:underline">(row.email.clone())</a>)
            table_cell((format!("{} {}", row.first_name, row.last_name)))
            table_cell(<span class="font-mono text-xs">(row.customer_id.clone())</span>)
        )
    })
}
