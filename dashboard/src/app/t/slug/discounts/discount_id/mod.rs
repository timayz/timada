//! `/t/{slug}/discounts/{discount_id}`: one discount, read from its own
//! stream (so an edit shows at once), with the forms that change it.

pub mod archive;
pub mod deactivate;
pub mod reactivate;

use timada_discounts::{DiscountState, DiscountStatus, unix_now};
use timada_tenancy::{PERMISSION_DISCOUNTS_MANAGE, PERMISSION_DISCOUNTS_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, view},
};

use super::{describe_limits, describe_window, discounts, discounts_url, error_code};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::auth::user_labels;
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::ui::{
    Shell, Tone, document, fact, facts, format_time, notice, page_header, section_card,
    status_badge,
};

module_param!(pub discount_id);

#[topcoat::router::query_params(error = bad_request)]
struct DiscountQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn discount_url(
    cx: &Cx,
    slug: &str,
    discount_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        DiscountId(discount_id.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The discount named in the URL, or 404.
async fn require_discount(cx: &Cx, access: &TenantAccess) -> Result<DiscountState> {
    let id = path_param::<DiscountId>(cx);
    discounts(access)?
        .load_discount(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// What every action on a discount shares: the member may manage discounts,
/// and the outcome goes back to the discount page (to the list once it is
/// archived, since an archived discount has nothing left to show).
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_discounts::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), timada_discounts::DiscountError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_DISCOUNTS_MANAGE)
        .await?;
    let discount_id = path_param::<DiscountId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = discounts(&access)?;
    match run(&gateway, &access.user_id, &discount_id).await {
        Ok(()) => Ok(see_other(discount_url(
            cx,
            &slug,
            &discount_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(discount_url(
            cx,
            &slug,
            &discount_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

/// Where a discount stands, in words.
fn describe_status(discount: &DiscountState, now: u64) -> &'static str {
    match discount.status {
        DiscountStatus::Archived => "Archived",
        DiscountStatus::Inactive => "Deactivated",
        DiscountStatus::Active if discount.starts_at.is_some_and(|start| now < start) => {
            "Not started yet"
        }
        DiscountStatus::Active if discount.ends_at.is_some_and(|end| end <= now) => "Over",
        DiscountStatus::Active => "Active",
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_DISCOUNTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_DISCOUNTS_MANAGE).await?;
    let query = query_params::<DiscountQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let discount = require_discount(cx, &access).await?;
    let now = unix_now();
    // How much of the cap is gone: the uses orders spent, and the ones
    // checkouts hold while their shoppers pay.
    let usage = match discount.max_uses {
        Some(_) => discounts(&access)?
            .load_usage(&discount.id)
            .await
            .map_err(topcoat::Error::from_anyhow)?
            .map(|usage| {
                let spent = usage.consumed_count as usize;
                (spent, usage.taken_at(now) - spent)
            }),
        None => None,
    };
    let labels = user_labels(cx, std::slice::from_ref(&discount.created_by)).await?;
    let created_by = labels
        .get(&discount.created_by)
        .cloned()
        .unwrap_or_else(|| discount.created_by.clone());
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let id = discount.id.clone();
    let title = format!("{} · Discounts · {}", discount.code, tenant.name);
    let status = describe_status(&discount, now);
    let window = describe_window(discount.starts_at, discount.ends_at);
    let limits = describe_limits(discount.max_uses, discount.once_per_email);
    let value = discount.value.to_string();
    let (spent, held) = usage.unwrap_or_default();
    let back = discounts_url(cx, &slug, None, None);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "discounts"),
            page_header(
                title: &discount.code,
                subtitle: Some("One discount code, read from its own record."),
                h1_attrs: attributes! { data-discount-code="" },
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"All discounts"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                facts(
                    fact(
                        term: "Status",
                        attrs: attributes! { data-discount-status=(discount.status.as_str()) },
                        status_badge(tone: status_tone(&discount, status), (status))
                    )
                    fact(term: "Takes off", attrs: attributes! { data-discount-value="" }, (value) " of the items")
                    fact(term: "When", (window))
                    fact(term: "Limits", (limits))
                    if let Some(max) = discount.max_uses {
                        fact(
                            term: "Used",
                            attrs: attributes! { data-discount-used=(spent) },
                            (spent) " of " (max)
                            if held > 0 {
                                <span class="text-muted-foreground">" (" (held) " held by checkouts being paid)"</span>
                            }
                        )
                    }
                    fact(term: "Created", (format_time(discount.created_at)) " by " (created_by))
                )
            )

            if can_manage && !discount.is_archived() {
                section_card(
                    title: Some("Actions"),
                    description: Some("Archiving frees the code for a new discount. A checkout already being paid keeps the discount it was shown."),
                    <div class="flex flex-wrap gap-3">
                        if discount.status == DiscountStatus::Active {
                            <form method="post" action=(href!(deactivate::deactivate, Slug(slug.clone()), DiscountId(id.clone())))>
                                button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Deactivate")
                            </form>
                        } else {
                            <form method="post" action=(href!(reactivate::reactivate, Slug(slug.clone()), DiscountId(id.clone())))>
                                button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Reactivate")
                            </form>
                        }
                        <form method="post" action=(href!(archive::archive, Slug(slug.clone()), DiscountId(id.clone())))>
                            button(variant: ButtonVariant::Destructive, attrs: attributes! { type="submit" }, "Archive")
                        </form>
                    </div>
                )
            }
        )
    })
}

/// The colour of where a discount stands.
fn status_tone(discount: &DiscountState, status: &str) -> Tone {
    match discount.status {
        DiscountStatus::Archived => Tone::Neutral,
        DiscountStatus::Inactive => Tone::Warning,
        DiscountStatus::Active if status == "Active" => Tone::Success,
        DiscountStatus::Active => Tone::Info,
    }
}
