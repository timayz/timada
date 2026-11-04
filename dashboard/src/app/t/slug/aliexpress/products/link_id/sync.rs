use std::sync::Arc;
use std::time::Duration;

use timada_aliexpress::{AliexpressError, Command, LinkState};
use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{Back, LinkId, back_to};
use crate::aliexpress::Aliexpress;
use crate::app::t::slug::require_tenant;
use crate::cluster::Executor;

/// How long the page waits for the read it asked for, which any node may
/// make; past that, the merchant is told it is on its way.
const READ_WAIT: Duration = Duration::from_secs(10);
/// A write to the store wakes the wait; this covers a missed one.
const READ_POLL: Duration = Duration::from_millis(250);

/// Asks for the product to be read again from AliExpress now, and waits for
/// the read so the page shows what it found.
#[route(POST)]
pub async fn sync(cx: &Cx, Form(form): Form<Back>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link_id = path_param::<LinkId>(cx).to_owned();
    let not_connected = |cx: &Cx| {
        back_to(
            cx,
            &access,
            &form.back,
            Some("aliexpress_not_connected"),
            None,
        )
    };
    let connected = app_context::<Arc<Aliexpress>>(cx)
        .connected(&access.tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if !connected {
        return Ok(not_connected(cx));
    }
    let links = Command::new(access.executor()?.clone(), &access.tenant.id);
    let asked = match links.request_sync(&access.user_id, &link_id).await {
        Ok(asked) => asked,
        Err(AliexpressError::NotFound) => return Ok(not_connected(cx)),
        Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
    };
    let read = read_after(&links, &link_id, asked)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let (error, message) = match &read {
        None => (None, Some("Reading it again from AliExpress.")),
        Some(link) if !link.is_linked() => (
            None,
            Some("The product is archived: no longer kept in step with AliExpress."),
        ),
        Some(link) if link.last_error.is_some() => (Some("aliexpress_sync_failed"), None),
        Some(link) if link.offline => {
            (None, Some("AliExpress no longer sells it: taken off sale."))
        }
        Some(_) => (None, Some("Read again from AliExpress.")),
    };
    Ok(back_to(cx, &access, &form.back, error, message))
}

/// The link once a read asked for by a member came after the request at
/// version `asked`, or the link is over. Any member's will do: it read the same
/// thing. The dashboard's own reads and requests do not count, since this one
/// may still be on its way. `None` when nothing came in time.
async fn read_after(
    links: &Command<Executor>,
    link_id: &str,
    asked: u16,
) -> anyhow::Result<Option<LinkState>> {
    let mut watch = evento::Executor::write_watch(links.executor());
    let deadline = tokio::time::Instant::now() + READ_WAIT;
    loop {
        if let Some(link) = links.load_link_by_id(link_id).await?
            && (link.answered_version > asked || !link.is_linked())
        {
            return Ok(Some(link));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        match watch.as_mut() {
            Some(watch) => {
                let _ = tokio::time::timeout(READ_POLL, watch.changed()).await;
            }
            None => tokio::time::sleep(READ_POLL).await,
        }
    }
}
