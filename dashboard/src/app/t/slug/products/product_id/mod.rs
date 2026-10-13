//! `/t/{slug}/products/{product_id}`: one product, read from its own stream
//! (so an edit shows at once), with the forms that change it.

pub mod archive;
pub mod categorize;
pub mod describe;
pub mod images;
pub mod price;
pub mod publish;
pub mod rename;
pub mod uncategorize;
pub mod unpublish;

use std::sync::Arc;
use timada_catalog::{
    CURRENCIES, CategoryState, MAX_ALT_LEN, MAX_DESCRIPTION_LEN, MAX_IMAGES_PER_PRODUCT,
    MAX_NAME_LEN, ProductState,
};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, view},
};

use super::{catalog, error_code, status_label};
use crate::app::auth::Flash;
use crate::app::t::slug::categories::{Choice, rows_of};
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::auth::user_labels;
use crate::db::ReadPool;
use crate::media::Media;
use crate::ui::{document, format_time, notice};

module_param!(pub product_id);

#[topcoat::router::query_params(error = bad_request)]
struct ProductQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn product_url(
    cx: &Cx,
    slug: &str,
    product_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        ProductId(product_id.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The product named in the URL, or 404.
async fn require_product(cx: &Cx, access: &TenantAccess) -> Result<ProductState> {
    let id = path_param::<ProductId>(cx);
    catalog(access)?
        .load_product(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// What every action on a product shares: the member may manage products,
/// the product exists, and the outcome goes back to the product page.
pub(crate) async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_catalog::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), timada_catalog::CatalogError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let product_id = path_param::<ProductId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = catalog(&access)?;
    match run(&gateway, &access.user_id, &product_id).await {
        Ok(()) => Ok(see_other(product_url(
            cx,
            &slug,
            &product_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(product_url(
            cx,
            &slug,
            &product_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

/// The categories a product is in, live ones first: read from their own
/// streams so the page shows a change the moment it is made.
async fn memberships_of(
    access: &TenantAccess,
    product: &ProductState,
) -> Result<Vec<CategoryState>> {
    let gateway = catalog(access)?;
    let mut memberships: Vec<CategoryState> = Vec::new();
    for category_id in &product.categories {
        if let Some(category) = gateway
            .load_category(category_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        {
            memberships.push(category);
        }
    }
    memberships
        .sort_by(|a, b| (a.archived, a.position, &a.name).cmp(&(b.archived, b.position, &b.name)));
    Ok(memberships)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<ProductQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let product = require_product(cx, &access).await?;
    let labels = user_labels(cx, std::slice::from_ref(&product.created_by)).await?;
    let created_by = labels
        .get(&product.created_by)
        .cloned()
        .unwrap_or_else(|| product.created_by.clone());
    let pool = app_context::<ReadPool>(cx);
    // Which categories it is in comes from the product's own stream, like
    // the rest of this page, so a change shows at once; each one is then
    // read from its own stream too (a product is in a handful at most).
    let memberships = memberships_of(&access, &product).await?;
    // Each picture with the address a browser loads it from.
    let media = app_context::<Arc<Media>>(cx);
    let gallery: Vec<(timada_catalog::ProductImage, String)> = product
        .images
        .iter()
        .map(|image| (image.clone(), media.url(&access.tenant.id, &image.file())))
        .collect();
    let gallery_full = gallery.len() >= MAX_IMAGES_PER_PRODUCT;
    let last = gallery.len().saturating_sub(1);
    // What it may still join is the whole tree, which the read model holds.
    let joinable: Vec<Choice> = rows_of(pool, &access.tenant.id)
        .await?
        .iter()
        .filter(|row| !product.categories.contains(&row.category.id))
        .map(Choice::of)
        .collect();
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let editable = can_manage && !product.is_archived();
    let current_currency = product
        .price
        .as_ref()
        .map(|p| p.currency.clone())
        .unwrap_or_else(|| CURRENCIES[0].to_owned());
    let current_amount = product
        .price
        .as_ref()
        .map(|p| format!("{}.{:02}", p.amount_cents / 100, p.amount_cents % 100))
        .unwrap_or_default();
    let title = format!("{} · Products · {}", product.name, tenant.name);
    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold" data-product-id=(&product.id)>
                    (&product.name)
                    " " <span class="rounded bg-neutral-100 px-2 text-sm font-normal text-neutral-700" data-product-status=(product.status.as_str())>(status_label(product.status))</span>
                </h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Overview"</h2>
                <dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                    <dt class="text-neutral-500">"Slug"</dt><dd><code>(&product.slug)</code></dd>
                    <dt class="text-neutral-500">"Price"</dt>
                    <dd data-product-price="">(product.price.as_ref().map(|p| p.to_string()).unwrap_or_else(|| "Not priced yet".to_owned()))</dd>
                    <dt class="text-neutral-500">"Description"</dt>
                    <dd data-product-description="" class="whitespace-pre-line">(if product.description.is_empty() { "—" } else { product.description.as_str() })</dd>
                    <dt class="text-neutral-500">"Created"</dt>
                    <dd>(&created_by) ", " <time datetime=(product.created_at)>(format_time(product.created_at))</time></dd>
                    <dt class="text-neutral-500">"Changed"</dt>
                    <dd><time datetime=(product.updated_at)>(format_time(product.updated_at))</time></dd>
                </dl>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Pictures"</h2>
                if gallery.is_empty() {
                    <p class="text-sm text-neutral-500">"No picture yet. The first one is what a listing shows."</p>
                }
                <ul class="grid gap-4 sm:grid-cols-2">
                    for (position, (image, url)) in gallery.iter().enumerate() {
                        <li class="flex gap-3 rounded border border-neutral-200 p-3" data-image=(&image.id)>
                            <img class="h-24 w-24 shrink-0 rounded object-cover" src=(url) alt=(&image.alt)
                                width=(image.width.unwrap_or(96)) height=(image.height.unwrap_or(96)) loading="lazy">
                            <div class="min-w-0 flex-1 text-sm">
                                if position == 0 {
                                    <p class="mb-1 text-xs text-neutral-500" data-image-first="">"Shown in listings"</p>
                                }
                                if editable {
                                    <form method="post" action=(href!(images::image_id::alt::alt, Slug(slug.clone()), ProductId(product.id.clone()), images::image_id::ImageId(image.id.clone())))>
                                        <label class="sr-only" for=(format!("alt-{}", image.id))>"What it shows"</label>
                                        <input id=(format!("alt-{}", image.id)) name="alt" type="text" maxlength=(MAX_ALT_LEN)
                                            class="w-full rounded border border-neutral-300 px-2 py-1" placeholder="What it shows" value=(&image.alt)>
                                        <button class="mt-2 rounded border border-neutral-300 px-2 py-1 text-xs">"Save"</button>
                                    </form>
                                    <div class="mt-2 flex gap-2">
                                        if position > 0 {
                                            <form method="post" action=(href!(images::image_id::up::up, Slug(slug.clone()), ProductId(product.id.clone()), images::image_id::ImageId(image.id.clone())))>
                                                <button class="rounded border border-neutral-300 px-2 py-1 text-xs" aria-label="Move earlier">"↑"</button>
                                            </form>
                                        }
                                        if position < last {
                                            <form method="post" action=(href!(images::image_id::down::down, Slug(slug.clone()), ProductId(product.id.clone()), images::image_id::ImageId(image.id.clone())))>
                                                <button class="rounded border border-neutral-300 px-2 py-1 text-xs" aria-label="Move later">"↓"</button>
                                            </form>
                                        }
                                        <form method="post" action=(href!(images::image_id::remove::remove, Slug(slug.clone()), ProductId(product.id.clone()), images::image_id::ImageId(image.id.clone())))>
                                            <button class="rounded border border-neutral-300 px-2 py-1 text-xs text-red-700">"Remove"</button>
                                        </form>
                                    </div>
                                } else {
                                    <p class="truncate text-neutral-500">(if image.alt.is_empty() { "—" } else { image.alt.as_str() })</p>
                                }
                            </div>
                        </li>
                    }
                </ul>
                if editable && !gallery_full {
                    <form method="post" enctype="multipart/form-data" class="mt-4 space-y-2"
                        action=(href!(images::upload::upload, Slug(slug.clone()), ProductId(product.id.clone())))>
                        <label class="block text-sm font-medium" for="product-image">"Add a picture"</label>
                        <input id="product-image" name="image" type="file" accept="image/jpeg,image/png,image/webp,image/gif" required="" class="block text-sm">
                        <label class="block text-sm" for="product-image-alt">"What it shows (optional)"</label>
                        <input id="product-image-alt" name="alt" type="text" maxlength=(MAX_ALT_LEN)
                            class="w-full rounded border border-neutral-300 px-3 py-2" placeholder="A blue mug on a desk">
                        <button class="rounded border border-neutral-300 px-3 py-1 text-sm">"Upload"</button>
                    </form>
                } else if editable {
                    <p class="mt-4 text-sm text-neutral-500">"This product carries as many pictures as it can. Remove one to add another."</p>
                }
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Categories"</h2>
                if memberships.is_empty() {
                    <p class="text-sm text-neutral-500">"In no category yet. A shopper finds a product by browsing the categories it is in."</p>
                }
                <ul class="divide-y divide-neutral-100 text-sm">
                    for membership in &memberships {
                        <li class="flex items-center justify-between py-2" data-membership=(&membership.slug)>
                            <span>
                                if membership.archived {
                                    (&membership.name) " " <span class="rounded bg-neutral-100 px-2 text-xs text-neutral-700">"archived"</span>
                                } else {
                                    <a class="underline" href=(href!(crate::app::t::slug::categories::category_id::index, Slug(slug.clone()), crate::app::t::slug::categories::category_id::CategoryId(membership.id.clone())))>(&membership.name)</a>
                                }
                            </span>
                            if editable {
                                <form method="post" action=(href!(uncategorize::uncategorize, Slug(slug.clone()), ProductId(product.id.clone())))>
                                    <input type="hidden" name="category_id" value=(&membership.id)>
                                    <button class="rounded border border-neutral-300 px-2 py-1 text-xs">"Remove"</button>
                                </form>
                            }
                        </li>
                    }
                </ul>
                if editable && !joinable.is_empty() {
                    <form method="post" action=(href!(categorize::categorize, Slug(slug.clone()), ProductId(product.id.clone()))) class="mt-3 flex gap-2">
                        <label class="sr-only" for="product-category">"Category"</label>
                        <select id="product-category" name="category_id" class="w-full rounded border border-neutral-300 px-3 py-2" required="">
                            for choice in &joinable {
                                <option value=(&choice.id)>(&choice.label)</option>
                            }
                        </select>
                        <button class="rounded border border-neutral-300 px-3 py-2 text-sm">"Add to category"</button>
                    </form>
                }
            </section>

            if product.is_archived() {
                <p class="mb-8 text-sm text-neutral-500">"An archived product cannot change any more."</p>
            }

            if editable {
                <section class="mb-8 grid gap-4 md:grid-cols-2">
                    <form method="post" action=(href!(rename::rename, Slug(slug.clone()), ProductId(product.id.clone()))) class="rounded border border-neutral-200 bg-white p-4">
                        <label class="block text-sm font-medium" for="product-name">"Name"</label>
                        <input id="product-name" name="name" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" maxlength=(MAX_NAME_LEN) required="" value=(&product.name)>
                        <button class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm">"Rename"</button>
                    </form>
                    <form method="post" action=(href!(price::price, Slug(slug.clone()), ProductId(product.id.clone()))) class="rounded border border-neutral-200 bg-white p-4">
                        <label class="block text-sm font-medium" for="product-amount">"Price"</label>
                        <div class="mt-1 flex gap-2">
                            <input id="product-amount" name="amount" class="w-full rounded border border-neutral-300 px-3 py-2"
                                type="text" inputmode="decimal" required="" placeholder="12.50" value=(&current_amount)>
                            <label class="sr-only" for="product-currency">"Currency"</label>
                            <select id="product-currency" name="currency" class="rounded border border-neutral-300 px-3 py-2">
                                for currency in CURRENCIES {
                                    <option value=(*currency) selected=(current_currency == *currency)>(*currency)</option>
                                }
                            </select>
                        </div>
                        <button class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm">"Set price"</button>
                    </form>
                    <form method="post" action=(href!(describe::describe, Slug(slug.clone()), ProductId(product.id.clone()))) class="rounded border border-neutral-200 bg-white p-4 md:col-span-2">
                        <label class="block text-sm font-medium" for="product-description">"Description"</label>
                        <textarea id="product-description" name="description" rows="5" maxlength=(MAX_DESCRIPTION_LEN)
                            class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">(&product.description)</textarea>
                        <button class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm">"Save description"</button>
                    </form>
                </section>

                <section class="flex flex-wrap items-center gap-3">
                    if product.is_published() {
                        <form method="post" action=(href!(unpublish::unpublish, Slug(slug.clone()), ProductId(product.id.clone())))>
                            <button class="rounded border border-neutral-300 px-3 py-1 text-sm">"Unpublish"</button>
                        </form>
                    } else {
                        <form method="post" action=(href!(publish::publish, Slug(slug.clone()), ProductId(product.id.clone())))>
                            <button class="rounded bg-neutral-900 px-3 py-1 text-sm font-medium text-white hover:bg-neutral-700">"Publish"</button>
                        </form>
                    }
                    <form method="post" action=(href!(archive::archive, Slug(slug.clone()), ProductId(product.id.clone())))>
                        <button class="rounded border border-red-700 px-3 py-1 text-sm text-red-700 hover:bg-red-50">"Archive"</button>
                    </form>
                </section>
            }
        )
    })
}

#[derive(serde::Deserialize)]
pub struct RenameForm {
    pub name: String,
}

#[derive(serde::Deserialize)]
pub struct DescribeForm {
    #[serde(default)]
    pub description: String,
}

#[derive(serde::Deserialize)]
pub struct PriceForm {
    pub amount: String,
    pub currency: String,
}
