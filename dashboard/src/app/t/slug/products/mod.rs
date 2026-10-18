//! `/t/{slug}/products`: the catalog of a tenant, for members with
//! `products:read`; those with `products:manage` create and edit. The list
//! comes from the shared read model (a moment behind), a product's own page
//! reads its stream.

pub mod create;
pub mod product_id;

use timada_catalog::{
    CatalogError, Command, MAX_NAME_LEN, ProductFilter, categories_of_tenant, count_products,
    products_of_tenant,
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
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, filter_bar, format_time, notice, page_header, pager,
    section_card, status_badge,
};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct ProductsQuery {
    /// Matches the slug or the name.
    q: Option<String>,
    /// The id of a category to list only.
    category: Option<String>,
    /// 1-based.
    page: Option<i64>,
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
    page: Option<i64>,
}

pub fn products_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The catalog gateway over the tenant's own store (403 while suspended).
pub fn catalog(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command(access.executor()?.clone()))
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
    match err {
        CatalogError::InvalidName => "invalid_product_name",
        CatalogError::InvalidSlug => "invalid_product_slug",
        CatalogError::SlugTaken => "product_slug_taken",
        CatalogError::InvalidDescription => "invalid_description",
        CatalogError::InvalidAmount => "invalid_amount",
        CatalogError::InvalidCurrency => "invalid_currency",
        CatalogError::NotFound => "product_not_found",
        CatalogError::Archived => "product_archived",
        CatalogError::NotPriced => "product_not_priced",
        CatalogError::CategoryNotFound => "category_not_found",
        CatalogError::CategoryArchived => "category_archived",
        CatalogError::CategoryCycle => "category_cycle",
        CatalogError::CategoryTooDeep => "category_too_deep",
        CatalogError::TooManyCategories => "too_many_categories",
        CatalogError::TooManyImages => "too_many_images",
        CatalogError::InvalidImage => "unsupported_image",
        CatalogError::InvalidAlt => "invalid_alt",
        CatalogError::ImageNotFound => "image_not_found",
        CatalogError::InvalidImageOrder => "invalid_image_order",
        CatalogError::TooManyOptions => "too_many_options",
        CatalogError::InvalidOptionName => "invalid_option_name",
        CatalogError::OptionExists => "option_exists",
        CatalogError::InvalidOptionValues => "invalid_option_values",
        CatalogError::OptionNotFound => "option_not_found",
        CatalogError::OptionInUse => "option_in_use",
        CatalogError::OptionValueInUse => "option_value_in_use",
        CatalogError::TooManyVariants => "too_many_variants",
        CatalogError::InvalidSelection => "invalid_selection",
        CatalogError::VariantExists => "variant_exists",
        CatalogError::VariantNotFound => "variant_not_found",
        CatalogError::CurrencyMismatch => "currency_mismatch",
        CatalogError::InvalidTaxCategory => "invalid_tax_category",
        CatalogError::InvalidSku => "invalid_sku",
        CatalogError::SkuTaken => "sku_taken",
        CatalogError::InvalidAttribute => "invalid_attribute",
        CatalogError::TooManyAttributes => "too_many_attributes",
        CatalogError::InvalidStock => "invalid_stock",
        CatalogError::OutOfStock { .. } => "out_of_stock",
        CatalogError::Write(err) => timada_core::write_refusal(err).unwrap_or("failed"),
        CatalogError::Other(_) => "failed",
    }
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
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let filter = ProductFilter::default().search(q).category(category);
    let total = count_products(pool, &tenant.id, &filter).await?;
    let rows = products_of_tenant(
        pool,
        &tenant.id,
        &filter,
        (page_number - 1) * PAGE_SIZE,
        PAGE_SIZE,
    )
    .await?;
    let category_rows = categories_of_tenant(pool, &tenant.id).await?;
    let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: i64| {
            href!(index, Slug(slug.clone()))
                .query(ProductsLink {
                    q,
                    category,
                    page: Some(page),
                })
                .resolve(cx)
        }
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
                            empty_row(colspan: 6, text: "No product yet.")
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
                pager(
                    page: page_number,
                    pages: pages,
                    previous: (page_number > 1).then(|| page_link(page_number - 1)),
                    next: (page_number < pages).then(|| page_link(page_number + 1)),
                )
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
