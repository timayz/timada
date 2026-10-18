//! The invitation page. Signed in, it offers to join; signed out, it keeps
//! the token in a private cookie and offers to create an account or sign in,
//! after which every sign-in path comes back here. Joining is an explicit
//! POST, never a side effect of opening the link.

pub mod accept;

use timada_tenancy::{InvitationState, TenantState};
use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    router::{href, module_param, page, path_param, query_params},
    view::{View, attributes, class, view},
};

use crate::app::auth::{Flash, login_url, register_url};
use crate::app::t::slug::{Slug, role_label};
use crate::app::tenants::tenancy;
use crate::auth::{
    cookies::{InviteFlow, invite_flow},
    current_user, now,
};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::card::CARD;
use crate::ui::{LINK, auth_document, icons, notice};

module_param!(pub token);

#[topcoat::router::query_params(error = bad_request)]
struct InvitationQuery {
    error: Option<String>,
}

pub fn invitation_url(cx: &Cx, token: &str, error: Option<&str>) -> String {
    href!(index, Token(token.to_owned()))
        .query(Flash {
            error,
            message: None,
        })
        .resolve(cx)
}

/// The invitation a token opens and its tenant, whatever state the link is
/// in (the caller checks `is_open_at`).
pub async fn load_invitation(
    cx: &Cx,
    token: &str,
) -> Result<Option<(InvitationState, TenantState)>> {
    let tenancy = tenancy(cx);
    let Some(invitation) = tenancy
        .load_invitation_by_token(token)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Ok(None);
    };
    let Some(tenant) = tenancy
        .load_tenant(&invitation.tenant_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Ok(None);
    };
    Ok(Some((invitation, tenant)))
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = path_param::<Token>(cx);
    let query = query_params::<InvitationQuery>(cx)?;
    let user = current_user(cx).await;
    let loaded = load_invitation(cx, token).await?;

    // What the page offers: nothing, joining, or signing in first. A link
    // the signed-in user already went through, or whose tenant they already
    // belong to, points them at the tenant instead of refusing.
    enum Offer {
        Invalid,
        Member { slug: String },
        Join,
        SignIn { email: String },
    }
    let (offer, invitation, tenant) = match loaded {
        None => {
            invite_flow(cx).remove();
            (Offer::Invalid, None, None)
        }
        Some((invitation, tenant)) => {
            let open = invitation.is_open_at(now());
            let offer = match user {
                Some(user) if tenant.members.contains_key(&user.id) => Offer::Member {
                    slug: tenant.slug.clone(),
                },
                _ if !open => {
                    invite_flow(cx).remove();
                    Offer::Invalid
                }
                Some(_) => Offer::Join,
                None => {
                    invite_flow(cx)
                        .set(InviteFlow {
                            token: token.to_owned(),
                            issued_at: now(),
                        })
                        .commit()?;
                    Offer::SignIn {
                        email: invitation.email.clone(),
                    }
                }
            };
            (offer, Some(invitation), Some(tenant))
        }
    };
    let role = match (&invitation, &tenant) {
        (Some(invitation), Some(tenant)) => role_label(tenant, &invitation.role),
        _ => String::new(),
    };
    let tenant_name = tenant.as_ref().map(|t| t.name.clone()).unwrap_or_default();
    let state = match offer {
        Offer::Invalid => "invalid",
        Offer::Member { .. } => "member",
        Offer::Join => "open",
        Offer::SignIn { .. } => "sign-in",
    };

    Ok(view! {
        auth_document(
            title: "Invitation",
            <section class=(class!(CARD, "gap-5 px-6")) data-invitation=(state)>
                <div class="flex flex-col items-center gap-3 text-center">
                    <span class=(class!(
                        "flex size-12 items-center justify-center rounded-xl [&>svg]:size-6",
                        if matches!(offer, Offer::Invalid) { "bg-muted text-muted-foreground" } else { "bg-primary/10 text-primary" },
                    ))>
                        icon(data: icons::INVITATIONS)
                    </span>
                    <h1 class="text-2xl font-bold tracking-tight">"Invitation"</h1>
                    <p class="text-sm text-muted-foreground">
                        (match &offer {
                            Offer::Invalid => "This invitation is no longer valid.".to_owned(),
                            Offer::Member { .. } => format!("You are already a member of {tenant_name}."),
                            Offer::Join | Offer::SignIn { .. } => format!("You are invited to join {tenant_name} as {role}."),
                        })
                    </p>
                </div>
                notice(error: query.error.as_deref())

                match offer {
                    Offer::Invalid => {
                        <p class="text-center text-sm">"It may have been used, withdrawn, or it expired. Ask the person who invited you for a new one."</p>
                        <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Md)) href=(href!(crate::app::home))>"Go to the dashboard"</a>
                    }
                    Offer::Member { slug } => {
                        <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Md)) href=(href!(crate::app::t::slug::index, Slug(slug)))>"Open " (&tenant_name)</a>
                    }
                    Offer::Join => {
                        <form method="post" action=(href!(accept::accept, Token(token.to_owned()))) class="flex flex-col gap-2">
                            button(attrs: attributes! { type="submit" class="w-full" }, icon(data: icons::CHECK) "Accept the invitation")
                            <a class=(button_variants(ButtonVariant::Ghost, ButtonSize::Md)) href=(href!(crate::app::home))>"Not now"</a>
                        </form>
                        if let Some(user) = user {
                            <p class="text-center text-xs text-muted-foreground">
                                "You are signed in as " <strong>(&user.label)</strong> ". "
                                <a class=(LINK) href=(login_url(cx, None))>"Not you? Switch account"</a>
                            </p>
                        }
                    }
                    Offer::SignIn { email } => {
                        <p class="text-center text-sm">"Create an account or sign in to accept; you will come back here."</p>
                        <div class="flex flex-col gap-2">
                            <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Md)) href=(register_url(cx, None, Some(&email)))>"Create an account"</a>
                            <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Md)) href=(login_url(cx, None))>icon(data: icons::LOG_IN) "Sign in"</a>
                        </div>
                    }
                }
            </section>
        )
    })
}
