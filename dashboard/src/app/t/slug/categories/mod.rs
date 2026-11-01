//! `/t/{slug}/categories`: the category tree of a tenant, for members with
//! `products:read`; those with `products:manage` organise it. The tree comes
//! from the shared read model (a moment behind), a category's own page reads
//! its stream.

pub mod category_id;
pub mod create;

use timada_catalog::{
    CatalogError, CategoryRow, MAX_CATEGORY_NAME_LEN, categories_of_tenant, tree_of,
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
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::button;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{LINK, Shell, document, empty_row, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct CategoriesQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn categories_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The query-string code of a user-facing error on a category: the variants
/// a category shares with a product are worded for a category here, the rest
/// read the same either way.
pub fn error_code(err: &CatalogError) -> &'static str {
    match err {
        CatalogError::InvalidName => "invalid_category_name",
        CatalogError::InvalidSlug => "invalid_category_slug",
        CatalogError::SlugTaken => "category_slug_taken",
        CatalogError::InvalidDescription => "invalid_category_description",
        other => super::products::error_code(other),
    }
}

/// A category and how deep it sits. The two numbers a merchant needs are on
/// the row: `product_count`, filed under it, and `subtree_count`, what the
/// shop's page for it lists.
pub struct Row {
    pub category: CategoryRow,
    pub depth: usize,
}

impl Row {
    /// Whether its page shows more than what is filed directly under it.
    pub fn inherits(&self) -> bool {
        self.category.subtree_count != self.category.product_count
    }
}

/// The live categories of a tenant in tree order, each with its depth.
pub async fn rows_of(pool: &Db, tenant_id: &str) -> Result<Vec<Row>> {
    fn walk(nodes: Vec<timada_catalog::TreeOf<CategoryRow>>, depth: usize, out: &mut Vec<Row>) {
        for node in nodes {
            out.push(Row {
                category: node.item,
                depth,
            });
            walk(node.children, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(
        tree_of(
            categories_of_tenant(pool, tenant_id)
                .await
                .map_err(topcoat::Error::from_anyhow)?,
        ),
        0,
        &mut out,
    );
    Ok(out)
}

/// A category as an option of a parent `<select>`: owned, because the view
/// takes what it renders.
pub struct Choice {
    pub id: String,
    /// The name, indented by its depth.
    pub label: String,
}

impl Choice {
    pub fn of(row: &Row) -> Self {
        Self {
            id: row.category.id.clone(),
            label: format!("{}{}", "— ".repeat(row.depth), row.category.name),
        }
    }
}

/// How a row is indented in the table.
fn indent(depth: usize) -> &'static str {
    match depth {
        0 => "",
        1 => "pl-6",
        _ => "pl-12",
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<CategoriesQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<Db>(cx);
    let rows = rows_of(pool, &tenant.id).await?;
    let slug = tenant.slug.clone();
    let title = format!("Categories · {}", tenant.name);
    // A category at the deepest level takes no children of its own.
    let parents: Vec<Choice> = rows
        .iter()
        .filter(|row| row.depth + 2 <= timada_catalog::MAX_CATEGORY_DEPTH)
        .map(Choice::of)
        .collect();

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "categories"),
            page_header(title: "Categories", subtitle: Some("The tree shoppers browse the catalog by."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                <p class="mb-4 text-sm text-muted-foreground">
                    "Categories are what a shopper browses: a top-level one holds sub-categories, three levels deep at most. A product belongs to as many as fit it."
                </p>
                <p class="mb-4 text-sm text-muted-foreground">
                    "A category's page in the shop lists what is filed under it and under every sub-category below it, so a product in a sub-category counts for its parents too."
                </p>
                table(
                    table_header(
                        table_row(
                            table_head("Name")
                            table_head("Slug")
                            table_head("On its page")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 3, text: "No category yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-category=(&row.category.slug) data-category-depth=(row.depth) },
                                table_cell(
                                    attrs: attributes! { class=(indent(row.depth)) },
                                    <a class=(LINK) href=(href!(category_id::index, Slug(slug.clone()), category_id::CategoryId(row.category.id.clone())))>(&row.category.name)</a>
                                    if !row.category.in_menu {
                                        " " badge(variant: BadgeVariant::Secondary, attrs: attributes! { data-hidden-from-menu="" }, "Hidden from menu")
                                    }
                                )
                                table_cell(<code class="text-xs">(&row.category.slug)</code>)
                                table_cell(
                                    <span class="tabular-nums" data-category-count=(row.category.subtree_count)>(row.category.subtree_count)</span>
                                    if row.inherits() {
                                        <span class="text-muted-foreground" data-category-direct=(row.category.product_count)>
                                            " (" (row.category.product_count) " filed here)"
                                        </span>
                                    }
                                )
                            )
                        }
                    )
                )
            )

            if can_manage {
                section_card(
                    title: Some("Create a category"),
                    description: Some("A top-level category, or one under another. The slug derives from the name unless given."),
                    <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="new-category-name" }, "Name")
                            input(attrs: attributes! {
                                id="new-category-name" name="name" type="text"
                                maxlength=(MAX_CATEGORY_NAME_LEN) required="" placeholder="Cat bedding"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="new-category-parent" }, "Parent")
                            select(
                                attrs: attributes! { id="new-category-parent" name="parent_id" },
                                <option value="">"Top level"</option>
                                for parent in &parents {
                                    <option value=(&parent.id)>(&parent.label)</option>
                                }
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="new-category-slug" }, "Slug (optional)")
                            input(attrs: attributes! { id="new-category-slug" name="slug" type="text" placeholder="cat-bedding" })
                        )
                        <div>
                            button(attrs: attributes! { type="submit" }, "Create category")
                        </div>
                    </form>
                )
            }
        )
    })
}
