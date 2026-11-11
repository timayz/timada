//! `/t/{slug}/products`: the catalog of a tenant, for members with
//! `products:read`; those with `products:manage` create and edit. The list
//! comes from the shared read model (a moment behind), a product's own page
//! reads its stream.

pub mod create;
pub mod product_id;

use timada_catalog::{
    CatalogError, Command, MAX_NAME_LEN, ProductCursor, ProductFilter, ProductPage, ProductStatus,
    categories_of_tenant, count_products, products_of_tenant,
};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, Tone, cursor_pager, document, empty_row, filter_bar, format_time, notice,
    page_header, section_card, status_badge,
};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct ProductsQuery {
    /// Matches the slug or the name.
    q: Option<String>,
    /// The id of a category to list only.
    category: Option<String>,
    /// One of the stored status values, or `all` for every status; absent
    /// (or anything else) lists the published products.
    status: Option<String>,
    /// The cursor of the row the page starts after (older products).
    after: Option<String>,
    /// The cursor of the row the page ends before (newer products).
    before: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct ProductsLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
}

/// The value of the status filter that lists every product.
const EVERY_STATUS: &str = "all";

/// What the `status` query parameter asks for: `None` is every status; the
/// default, absent or unknown, is the published products.
fn status_filter(param: Option<&str>) -> Option<ProductStatus> {
    match param.map(str::trim) {
        Some(EVERY_STATUS) => None,
        Some(value) => Some(ProductStatus::parse(value).unwrap_or(ProductStatus::Published)),
        None => Some(ProductStatus::Published),
    }
}

/// The empty-table line for what was asked.
fn empty_text(filter: &ProductFilter<'_>) -> &'static str {
    if filter.q.is_some() || filter.category.is_some() {
        return "No product matches.";
    }
    match filter.status {
        Some(ProductStatus::Published) => "No published product yet.",
        Some(ProductStatus::Draft) => "No draft.",
        Some(ProductStatus::Archived) => "No archived product.",
        None => "No product yet.",
    }
}

pub fn products_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The catalog gateway in the tenant's streams (403 while suspended).
pub fn catalog(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
}

/// How a listing words a product's stock.
pub fn stock_label(state: &str) -> &'static str {
    match timada_catalog::StockSummary::parse(state) {
        timada_catalog::StockSummary::Untracked => "Not counted",
        timada_catalog::StockSummary::InStock => "In stock",
        timada_catalog::StockSummary::SoldOut => "Sold out",
    }
}

/// The query-string code of a user-facing catalog error.
pub fn error_code(err: &CatalogError) -> &'static str {
    err.code().unwrap_or("failed")
}

/// The word for a product's state on the pages.
pub fn status_label(status: timada_catalog::ProductStatus) -> &'static str {
    match status {
        timada_catalog::ProductStatus::Draft => "Draft",
        timada_catalog::ProductStatus::Published => "Published",
        timada_catalog::ProductStatus::Archived => "Archived",
    }
}

/// The colour of a product's state.
pub fn status_tone(status: timada_catalog::ProductStatus) -> Tone {
    match status {
        timada_catalog::ProductStatus::Draft => Tone::Info,
        timada_catalog::ProductStatus::Published => Tone::Success,
        timada_catalog::ProductStatus::Archived => Tone::Neutral,
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<ProductsQuery>(cx)?;
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let category = query
        .category
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let status = status_filter(query.status.as_deref());
    // What the links carry: the parameter as given, when it was.
    let status_param = query.status.as_deref().map(str::trim);
    let after = query.after.as_deref().and_then(ProductCursor::parse);
    let before = query.before.as_deref().and_then(ProductCursor::parse);
    let page = match (&after, &before) {
        (Some(after), _) => ProductPage::Older(after),
        (None, Some(before)) => ProductPage::Newer(before),
        (None, None) => ProductPage::First,
    };
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<Db>(cx);
    let filter = ProductFilter::default()
        .search(q)
        .category(category)
        .status(status);
    let total = count_products(pool, &tenant.id, &filter)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let listed = products_of_tenant(pool, &tenant.id, &filter, page, PAGE_SIZE)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = listed.rows;
    let category_rows = categories_of_tenant(pool, &tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |after: Option<&str>, before: Option<&str>| {
            href!(index, Slug(slug.clone()))
                .query(ProductsLink {
                    q,
                    category,
                    status: status_param,
                    after,
                    before,
                })
                .resolve(cx)
        }
    };
    let newer = listed
        .newer
        .map(|cursor| page_link(None, Some(&cursor.param())));
    let older = listed
        .older
        .map(|cursor| page_link(Some(&cursor.param()), None));
    let summary = match total {
        1 => "1 product".to_owned(),
        n => format!("{n} products"),
    };
    let title = format!("Products · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "products"),
            page_header(title: "Products", subtitle: Some("The catalog shoppers browse."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                filter_bar(
                    <label class="sr-only" for="products-search">"Search"</label>
                    input(attrs: attributes! {
                        id="products-search" name="q" type="search" value=(q.unwrap_or(""))
                        class="sm:max-w-xs" placeholder="Name or slug"
                    })
                    if !category_rows.is_empty() {
                        <label class="sr-only" for="products-category">"Category"</label>
                        select(
                            attrs: attributes! { id="products-category" name="category" class="w-48" },
                            <option value="">"Every category"</option>
                            for row in &category_rows {
                                <option value=(&row.id) selected=(category == Some(row.id.as_str()))>(&row.name)</option>
                            }
                        )
                    }
                    <label class="sr-only" for="products-status">"Status"</label>
                    select(
                        attrs: attributes! { id="products-status" name="status" class="w-44" },
                        for option in ProductStatus::ALL {
                            <option value=(option.as_str()) selected=(status == Some(option))>(status_label(option))</option>
                        }
                        <option value=(EVERY_STATUS) selected=(status.is_none())>"Every status"</option>
                    )
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Search")
                )
                table(
                    table_header(
                        table_row(
                            table_head("Name")
                            table_head("Slug")
                            table_head(attrs: attributes! { class="text-right" }, "Price")
                            table_head("Stock")
                            table_head("Status")
                            table_head("Changed")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 6, text: empty_text(&filter))
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-product=(&row.slug) data-product-status=(&row.status) },
                                table_cell(
                                    <a class=(LINK) href=(href!(product_id::index, Slug(slug.clone()), product_id::ProductId(row.id.clone())))>(&row.name)</a>
                                )
                                table_cell(<code class="text-xs">(&row.slug)</code>)
                                table_cell(attrs: attributes! { class="text-right tabular-nums" data-product-price="" }, (row.price_label().unwrap_or_else(|| "—".to_owned())))
                                table_cell(attrs: attributes! { data-product-stock=(&row.stock_state) }, (stock_label(&row.stock_state)))
                                table_cell(status_badge(tone: status_tone(row.status()), (status_label(row.status()))))
                                table_cell(<time datetime=(row.updated_at)>(format_time(row.updated_at as u64))</time>)
                            )
                        }
                    )
                )
                cursor_pager(summary: &summary, newer: newer, older: older)
            )

            if can_manage {
                section_card(
                    title: Some("Create a product"),
                    description: Some("A new product is a draft: set its price and publish it from its page. The slug derives from the name unless given."),
                    <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="new-product-name" }, "Name")
                            input(attrs: attributes! {
                                id="new-product-name" name="name" type="text"
                                maxlength=(MAX_NAME_LEN) required="" placeholder="Blue mug"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="new-product-slug" }, "Slug (optional)")
                            input(attrs: attributes! { id="new-product-slug" name="slug" type="text" placeholder="blue-mug" })
                        )
                        <div>
                            button(attrs: attributes! { type="submit" }, "Create product")
                        </div>
                    </form>
                )
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store not taking writes for the moment is told to the person as
    /// `writes_paused`; any other write failure stays the generic code.
    #[test]
    fn a_paused_store_has_its_own_code() {
        let paused = CatalogError::Write(evento::WriteError::Unavailable(anyhow::anyhow!(
            "backlog at cap"
        )));
        assert!(paused.is_user_facing());
        assert_eq!(error_code(&paused), timada_core::WRITES_PAUSED);
        let broken = CatalogError::Write(evento::WriteError::Unknown(anyhow::anyhow!("disk gone")));
        assert!(!broken.is_user_facing());
        assert_eq!(error_code(&broken), "failed");
    }
}
