//! `/t/{slug}/shipping/{method_id}`: one shipping method, with the form that
//! changes it and the one that removes it.

pub mod change;
pub mod remove;

use timada_catalog::CURRENCIES;
use timada_orders::{MAX_SHIPPING_NAME_LEN, OrderError, ShippingMethod};
use timada_tenancy::PERMISSION_SHIPPING_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, view},
};

use super::{amount_field, countries_field, error_code, orders, shipping_url};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::ui::{Shell, document, notice, page_header, section_card};

module_param!(pub method_id);

#[topcoat::router::query_params(error = bad_request)]
struct MethodQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn method_url(
    cx: &Cx,
    slug: &str,
    method_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, Slug(slug.to_owned()), MethodId(method_id.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The method named in the URL, or 404.
async fn require_method(cx: &Cx, access: &TenantAccess) -> Result<ShippingMethod> {
    let id = path_param::<MethodId>(cx);
    orders(access)?
        .shipping_methods()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .into_iter()
        .find(|method| method.id == *id)
        .ok_or_else(|| not_found().into())
}

/// What every action on a method shares: the member may manage shipping,
/// and a refusal goes back to the method's page. Done, it goes to the list.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_orders::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), OrderError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SHIPPING_MANAGE)
        .await?;
    let method_id = path_param::<MethodId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = orders(&access)?;
    match run(&gateway, &access.user_id, &method_id).await {
        Ok(()) => Ok(see_other(shipping_url(cx, &slug, None, Some(message)))),
        Err(OrderError::ShippingMethodNotFound) => Ok(see_other(shipping_url(
            cx,
            &slug,
            Some("shipping_method_not_found"),
            None,
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(method_url(
            cx,
            &slug,
            &method_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SHIPPING_MANAGE)
        .await?;
    let query = query_params::<MethodQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let method = require_method(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let id = method.id.clone();
    let title = format!("{} · Shipping · {}", method.name, tenant.name);
    let back = shipping_url(cx, &slug, None, None);
    let price = amount_field(method.price_cents);
    let free_above = method
        .free_above_cents
        .map(amount_field)
        .unwrap_or_default();

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "shipping"),
            page_header(
                title: &method.name,
                subtitle: Some("One way the shop ships."),
                h1_attrs: attributes! { data-shipping-name="" },
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"All shipping methods"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Settings"),
                <form method="post" action=(href!(change::change, Slug(slug.clone()), MethodId(id.clone()))) class="flex max-w-xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="shipping-name" }, "Name")
                        input(attrs: attributes! {
                            id="shipping-name" name="name" type="text"
                            maxlength=(MAX_SHIPPING_NAME_LEN) required="" value=(&method.name)
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="shipping-price" }, "Price")
                        <div class="flex gap-2">
                            input(attrs: attributes! {
                                id="shipping-price" name="price" type="text" inputmode="decimal" required="" value=(&price)
                            })
                            select(
                                attrs: attributes! { name="currency" aria-label="Currency" class="w-28 shrink-0" },
                                for currency in CURRENCIES {
                                    <option value=(*currency) selected=(method.currency == *currency)>(*currency)</option>
                                }
                            )
                        </div>
                    )
                    field(
                        field_label(attrs: attributes! { for="shipping-free" }, "Free from (optional)")
                        input(attrs: attributes! {
                            id="shipping-free" name="free_above" type="text" inputmode="decimal" value=(&free_above)
                        })
                    )
                    countries_field(id: "shipping-countries", selected: &method.countries)
                    <div>
                        button(attrs: attributes! { type="submit" }, "Save shipping method")
                    </div>
                </form>
            )

            section_card(
                title: Some("Remove this method"),
                description: Some("A shopper who picked it and has not paid yet is asked to pick another. Orders already placed keep what they paid."),
                <form method="post" action=(href!(remove::remove, Slug(slug.clone()), MethodId(id.clone())))>
                    button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Remove")
                </form>
            )
        )
    })
}
