use serde::Deserialize;
use timada_tenancy::InviteMember;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, route,
    },
};

use super::members_url;
use crate::app::invitations::token::{Token, index as invitation_page};
use crate::app::t::slug::{require_tenant, role_label};
use crate::app::tenants::{error_code, tenancy};
use crate::auth::{mail::send_invitation, now, require_user};

#[derive(Deserialize)]
struct InviteForm {
    email: String,
    role: String,
}

/// Records an invitation and mails its link. When the mail cannot go out the
/// invitation is withdrawn again (best effort), so a retry sends a fresh one.
#[route(POST)]
pub async fn invite(cx: &Cx, Form(form): Form<InviteForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    let tenancy = tenancy(cx);
    let sent = match tenancy
        .invite_member(
            &access.user_id,
            InviteMember {
                tenant_id: access.tenant.id.clone(),
                email: form.email,
                role: form.role,
            },
            now(),
        )
        .await
    {
        Ok(sent) => sent,
        Err(err) if err.is_user_facing() => {
            return Ok(see_other(members_url(
                cx,
                slug,
                Some(error_code(&err)),
                None,
            )));
        }
        Err(err) => return Err(err.into()),
    };

    let link = href!(invitation_page, Token(sent.token.clone()))
        .absolute()
        .resolve(cx);
    let role = role_label(&access.tenant, &sent.invitation.role);
    if let Err(err) = send_invitation(
        cx,
        &sent.invitation.email,
        &access.tenant.name,
        &user.label,
        &role,
        &link,
    )
    .await
    {
        tracing::warn!(%err, invitation = %sent.invitation.id, "cannot mail the invitation");
        if let Err(revoke) = tenancy
            .revoke_invitation(&access.user_id, &sent.invitation.id, now())
            .await
        {
            tracing::warn!(%revoke, invitation = %sent.invitation.id, "cannot withdraw the unsent invitation");
        }
        return Ok(see_other(members_url(cx, slug, Some("mail_failed"), None)));
    }
    Ok(see_other(members_url(
        cx,
        slug,
        None,
        Some(&format!("Invitation sent to {}.", sent.invitation.email)),
    )))
}
