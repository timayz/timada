use timada_identity::SignInPasskey;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Json,
        error::{bad_request, unauthorized},
        route,
    },
};
use webauthn_rs::prelude::{DiscoverableKey, PublicKeyCredential, Webauthn};

use crate::app::auth::passkey::{Done, into_error};
use crate::app::auth::signed_in_url;
use crate::auth::{
    cookies::{is_fresh, passkey_login_flow},
    current_user, identity, login_context, now, sign_in_as,
    webauthn::{passkey_from_json, passkey_to_json},
};

#[route(POST)]
pub async fn finish(cx: &Cx, Json(credential): Json<PublicKeyCredential>) -> Result<Json<Done>> {
    let webauthn = app_context::<Webauthn>(cx);
    let store = passkey_login_flow(cx);
    let flow = match store.parse() {
        Ok(Some(store)) => {
            let flow = store.get();
            store.remove();
            flow
        }
        Ok(None) | Err(_) => return Err(bad_request("this sign-in attempt expired").into()),
    };
    if !is_fresh(flow.issued_at, now()) {
        return Err(bad_request("this sign-in attempt expired").into());
    }

    // Which credential signed? Non-discoverable keys carry no user handle and
    // fail here: they can register but cannot sign in without a username.
    let (_user_handle, credential_id) = webauthn
        .identify_discoverable_authentication(&credential)
        .map_err(|err| {
            tracing::debug!(%err, "credential is not usable for passwordless sign-in");
            bad_request("this passkey cannot be used for passwordless sign-in here")
        })?;
    let credential_id = crate::auth::webauthn::encode_credential_id(&credential_id.to_vec().into());

    let Some(stored) = identity(cx)
        .load_passkey(&credential_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        // Removed, or its account deleted: "not registered" is the honest answer.
        return Err(into_error(
            timada_identity::IdentityError::UnknownCredential,
        ));
    };
    let mut passkey =
        passkey_from_json(&stored.passkey_json).map_err(topcoat::Error::from_anyhow)?;
    let result = webauthn
        .finish_discoverable_authentication(
            &credential,
            flow.state,
            &[DiscoverableKey::from(&passkey)],
        )
        .map_err(|err| {
            tracing::warn!(%err, credential_id, "passkey assertion failed");
            unauthorized()
        })?;
    let updated = passkey.update_credential(&result) == Some(true);

    let outcome = identity(cx)
        .sign_in_passkey(SignInPasskey {
            credential_id,
            updated_passkey_json: if updated {
                Some(passkey_to_json(&passkey).map_err(topcoat::Error::from_anyhow)?)
            } else {
                None
            },
            session_user_id: current_user(cx).await.map(|u| u.id.clone()),
            context: login_context(cx),
        })
        .await
        .map_err(into_error)?;
    sign_in_as(cx, &outcome.user_id).await?;
    Ok(Json(Done {
        location: signed_in_url(cx, &outcome.label, outcome.switched_from.is_some()),
    }))
}
