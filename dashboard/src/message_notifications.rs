//! Tells the shop's members a contact message arrived: a platform mail to
//! every member whose role lets them read the messages, with a link to the
//! inbox. The mail carries the visitor's first name and nothing else they
//! wrote, so what the shop may later erase does not live on in mailboxes.

use std::sync::Arc;

use crate::db::Db;
use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::keys::KeyRing;
use timada_identity::UserRow;
use timada_support::aggregator::Received;
use timada_tenancy::{PERMISSION_SUPPORT_READ, TenantState};
use topcoat::context::{AppContext, Cx};

use crate::cluster::Executor as Store;
use crate::mail::send_message_notification;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/message-notifications";
/// Who the notices say mailed them.
pub const MAILER: &str = "timada-dashboard";

/// A message received longer ago than this is history being replayed (a
/// node catching up, or the first start of this subscription): the members
/// are not told now.
const NOTIFICATION_WINDOW_SECS: u64 = 60 * 60;

/// The platform store, where the shop and its members live.
#[derive(Clone)]
struct Platform(Store);

/// The shared read models, where the members' addresses are.
#[derive(Clone)]
struct ReadModels(Db);

/// The store, to open the message and claim its notice.
#[derive(Clone)]
struct TenantStore(Evento);

/// The platform's transport and sender.
#[derive(Clone)]
struct Mail(Arc<AppContext>);

/// The dashboard's public address, for the link.
#[derive(Clone)]
struct BaseUrl(String);

/// The tenant's keys, to read the visitor's first name.
#[derive(Clone)]
struct TenantKeys(Arc<dyn KeyRing>);

/// Follows every tenant's messages. Every node runs it;
/// the node that claims a message's notice first is the one that mails.
#[allow(clippy::too_many_arguments)]
pub fn subscription<E: Executor>(
    platform: Store,
    db: Db,
    mail: Arc<AppContext>,
    base_url: String,
    keys: Arc<dyn KeyRing>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(ReadModels(db))
        .data(Mail(mail))
        .data(BaseUrl(base_url))
        .data(TenantKeys(keys))
        .data(TenantStore(store))
        .handler(on_received())
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_received<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Received>,
) -> anyhow::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if now.saturating_sub(event.timestamp) > NOTIFICATION_WINDOW_SECS {
        return Ok(());
    }
    let store: TenantStore = context.extract();
    let support = timada_support::Command::new(store.0.clone(), &event.tenant);
    // Erased meanwhile: nothing to tell.
    let Some(state) = support.load_message(&event.aggregate_id).await? else {
        return Ok(());
    };
    // A key not here yet fails the handler, which is retried.
    let keys: TenantKeys = context.extract();
    let Some(message) = state.open_with(&*keys.0.for_tenant(&event.tenant)).await? else {
        return Ok(());
    };
    let platform: Platform = context.extract();
    let Some(shop) = timada_tenancy::Command(platform.0.clone())
        .load_tenant(&event.tenant)
        .await?
    else {
        return Ok(());
    };
    let read_models: ReadModels = context.extract();
    let recipients =
        mailboxes(timada_identity::users_by_ids(&read_models.0, &recipient_ids(&shop)).await?);
    if recipients.is_empty() {
        tracing::debug!(tenant = %event.tenant, message = %message.id, "no member to tell of a message");
        return Ok(());
    }
    if !support.claim_notification(&message.id, MAILER).await? {
        return Ok(());
    }
    // Claimed, then sent: a failure here is not retried, since members told
    // twice are worse off than one left to open the inbox.
    let mail: Mail = context.extract();
    let base_url: BaseUrl = context.extract();
    let cx = Cx::new(mail.0);
    let link = format!(
        "{}/t/{}/messages",
        base_url.0.trim_end_matches('/'),
        shop.slug
    );
    for to in &recipients {
        match send_message_notification(&cx, to, &shop.name, &message.first_name, &link).await {
            Ok(()) => {
                tracing::info!(tenant = %event.tenant, message = %message.id, "message notice mailed");
            }
            Err(err) => {
                tracing::error!(tenant = %event.tenant, message = %message.id, error = %err, "message notice not delivered");
            }
        }
    }
    Ok(())
}

/// The members who may read the messages: the owners, and those whose role
/// grants `support:read` (or implies it).
fn recipient_ids(shop: &TenantState) -> Vec<String> {
    shop.members
        .keys()
        .filter(|user_id| shop.can(user_id, PERMISSION_SUPPORT_READ))
        .cloned()
        .collect()
}

/// The addresses to mail among these users: the living ones that have one
/// (a member who signed up with a provider or a passkey alone has none).
fn mailboxes(users: Vec<UserRow>) -> Vec<String> {
    users
        .into_iter()
        .filter(|user| !user.deleted)
        .filter_map(|user| user.email)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use timada_tenancy::{PERMISSION_SUPPORT_MANAGE, ROLE_OWNER, RoleDef};

    use super::*;

    fn role(permissions: &[&str]) -> RoleDef {
        RoleDef {
            label: String::new(),
            permissions: permissions
                .iter()
                .map(|p| (*p).to_owned())
                .collect::<BTreeSet<_>>(),
        }
    }

    fn user(id: &str, email: Option<&str>, deleted: bool) -> UserRow {
        UserRow {
            id: id.to_owned(),
            account: id.to_owned(),
            label: id.to_owned(),
            is_admin: false,
            deleted,
            registered_at: 0,
            last_sign_in_at: None,
            sign_in_count: 0,
            oauth2_count: 0,
            passkey_count: 0,
            email: email.map(str::to_owned),
            has_password: email.is_some(),
        }
    }

    #[test]
    fn the_members_who_may_read_the_messages_are_told() {
        let mut shop = TenantState::default();
        shop.roles.insert("reader".into(), role(&["support:read"]));
        shop.roles
            .insert("agent".into(), role(&[PERMISSION_SUPPORT_MANAGE]));
        shop.roles.insert("clerk".into(), role(&["orders:read"]));
        for (user, role) in [
            ("owner", ROLE_OWNER),
            ("reader", "reader"),
            ("agent", "agent"),
            ("clerk", "clerk"),
            ("ghost", "no-such-role"),
        ] {
            shop.members.insert(user.into(), role.into());
        }
        assert_eq!(recipient_ids(&shop), vec!["agent", "owner", "reader"]);
    }

    #[test]
    fn only_living_members_with_an_address_are_mailed() {
        let mailed = mailboxes(vec![
            user("owner", Some("owner@example.com"), false),
            user("oidc-only", None, false),
            user("gone", Some("gone@example.com"), true),
        ]);
        assert_eq!(mailed, vec!["owner@example.com"]);
    }
}
