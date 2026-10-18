//! The invitation page. Signed in, it offers to join; signed out, it keeps
//! the token in a private cookie and offers to create an account or sign in,
//! after which every sign-in path comes back here. Joining is an explicit
//! POST, never a side effect of opening the link.

pub mod accept;

use timada_tenancy::{InvitationState, TenantState};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param, query_params},
    view::{View, view},
};

use crate::app::auth::{Flash, login_url, register_url};
use crate::app::t::slug::{Slug, role_label};
use crate::app::tenants::tenancy;
use crate::auth::{
    cookies::{InviteFlow, invite_flow},
    current_user, now,
};
use crate::ui::{Shell, document, notice};

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

    Ok(view! {
        document(
            title: "Invitation",
            shell: Shell::page(""),
            <h1 class="mb-6 text-2xl font-semibold">"Invitation"</h1>
            notice(error: query.error.as_deref())

            match offer {
                Offer::Invalid => {
                    <section class="rounded border border-neutral-200 bg-white p-4" data-invitation="invalid">
                        <p class="text-sm">"This invitation is no longer valid. It may have been used, withdrawn, or it expired. Ask the person who invited you for a new one."</p>
                        <p class="mt-3 text-sm"><a class="underline" href=(href!(crate::app::home))>"Dashboard"</a></p>
                    </section>
                }
                Offer::Member { slug } => {
                    <section class="rounded border border-neutral-200 bg-white p-4" data-invitation="member">
                        <p class="text-sm">"You are already a member of " <strong>(&tenant_name)</strong> "."</p>
                        <p class="mt-3 text-sm"><a class="underline" href=(href!(crate::app::t::slug::index, Slug(slug)))>"Open " (&tenant_name)</a></p>
                    </section>
                }
                Offer::Join => {
                    <section class="rounded border border-neutral-200 bg-white p-4" data-invitation="open">
                        <p class="text-sm">"You are invited to join " <strong>(&tenant_name)</strong> " as " <strong>(&role)</strong> "."</p>
                        <form method="post" action=(href!(accept::accept, Token(token.to_owned()))) class="mt-4 flex items-center gap-4">
                            <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Accept the invitation"</button>
                            <a class="text-sm underline" href=(href!(crate::app::home))>"Not now"</a>
                        </form>
                        if let Some(user) = user {
                            <p class="mt-4 text-xs text-neutral-500">
                                "You are signed in as " (&user.label) ". "
                                <a class="underline" href=(login_url(cx, None))>"Not you? Switch account"</a>
                            </p>
                        }
                    </section>
                }
                Offer::SignIn { email } => {
                    <section class="rounded border border-neutral-200 bg-white p-4" data-invitation="sign-in">
                        <p class="text-sm">"You are invited to join " <strong>(&tenant_name)</strong> " as " <strong>(&role)</strong> ". Create an account or sign in to accept; you will come back here."</p>
                        <p class="mt-4 flex items-center gap-4 text-sm">
                            <a class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700" href=(register_url(cx, None, Some(&email)))>"Create an account"</a>
                            <a class="underline" href=(login_url(cx, None))>"Sign in"</a>
                        </p>
                    </section>
                }
            }
        )
    })
}
