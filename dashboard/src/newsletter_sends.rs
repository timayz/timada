//! Sends the shops' newsletter campaigns: once a merchant sends one, every
//! node with the shop open mails its confirmed subscribers a batch at a time,
//! on a timer, from the shop. Each subscriber's mail is claimed first
//! (`Delivery`), so a subscriber gets a campaign once however many nodes
//! send it; a campaign with nobody left to mail is finished.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use timada_core::Keys;
use timada_newsletter::{CampaignStatus, sending_campaigns, undelivered_subscribers};
use tokio::task::JoinHandle;
use topcoat::context::{AppContext, Cx};

use crate::auth::mail::{CampaignCopy, send_campaign};
use crate::cluster::Executor;
use crate::key_vault::KeyVault;
use crate::newsletter::unsubscribe_link;
use crate::tenants::Tenants;

/// Who the deliveries say mailed them.
pub const MAILER: &str = "timada-dashboard";

/// The sweep, with what it needs outside any request.
pub struct NewsletterSends {
    read_pool: SqlitePool,
    platform: Executor,
    tenants: Arc<Tenants>,
    mail: Arc<AppContext>,
    keys: Arc<KeyVault>,
    batch: u32,
}

impl NewsletterSends {
    pub fn new(
        read_pool: SqlitePool,
        platform: Executor,
        tenants: Arc<Tenants>,
        mail: Arc<AppContext>,
        keys: Arc<KeyVault>,
        batch: u32,
    ) -> Arc<Self> {
        Arc::new(Self {
            read_pool,
            platform,
            tenants,
            mail,
            keys,
            batch: batch.max(1),
        })
    }

    /// Sweeps every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(err) = self.sweep().await {
                    tracing::error!(error = %err, "newsletter campaigns not swept");
                }
            }
        })
    }

    /// Sends a batch of every campaign going out. A campaign that fails is
    /// logged and the others still go.
    pub async fn sweep(&self) -> anyhow::Result<()> {
        for (tenant_id, campaign_id) in sending_campaigns(&self.read_pool).await? {
            if let Err(err) = self.sweep_campaign(&tenant_id, &campaign_id).await {
                tracing::error!(tenant = %tenant_id, campaign = %campaign_id, error = %err, "newsletter batch not sent");
            }
        }
        Ok(())
    }

    async fn sweep_campaign(&self, tenant_id: &str, campaign_id: &str) -> anyhow::Result<()> {
        let Some(tenant) = timada_tenancy::Command(self.platform.clone())
            .load_tenant(tenant_id)
            .await?
        else {
            return Ok(());
        };
        if tenant.is_suspended() {
            return Ok(());
        }
        let lease = self.tenants.lease(&tenant).await?;
        let cx = Cx::new(self.mail.clone());
        send_batch(
            lease.executor(),
            &self.read_pool,
            &cx,
            &tenant,
            &*self.keys.for_tenant(tenant_id),
            campaign_id,
            self.batch,
        )
        .await?;
        Ok(())
    }
}

/// Mails up to `batch` of the campaign's subscribers not mailed yet, then
/// finishes the campaign when nobody is left. Returns how many were mailed.
pub async fn send_batch<E: evento::Executor + Clone>(
    store: &E,
    read_pool: &SqlitePool,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    keys: &dyn Keys,
    campaign_id: &str,
    batch: u32,
) -> anyhow::Result<usize> {
    let newsletter = timada_newsletter::Command(store.clone());
    let Some(campaign) = newsletter.load_campaign(campaign_id).await? else {
        return Ok(0);
    };
    if campaign.status != CampaignStatus::Sending {
        return Ok(0);
    }
    // Without the storefront there is no unsubscribe link to put in the
    // mails: nothing goes out until the merchant says where it lives.
    let Some(storefront) = timada_orders::Command(store.clone())
        .storefront_address()
        .await?
    else {
        tracing::warn!(tenant = %tenant.id, campaign = %campaign.id, "newsletter held back: the shop has no storefront address");
        return Ok(0);
    };
    let owed = undelivered_subscribers(
        read_pool,
        &tenant.id,
        &campaign.id,
        campaign.language.as_deref(),
        batch,
    )
    .await?;
    let copy = CampaignCopy {
        subject: &campaign.subject,
        html: &campaign.html,
        text: campaign.text.as_deref(),
    };
    let mut mailed = 0;
    for row in &owed {
        // The read model is a moment behind: the write model says whether
        // they are still on the list.
        let Some(state) = newsletter.load_subscriber(&row.id).await? else {
            continue;
        };
        if !state.is_confirmed() {
            continue;
        }
        // Opened before the claim: a key not here yet leaves the mail for a
        // later sweep rather than claimed and never sent.
        let (Some(subscriber), Some(token)) = (
            state.open_with(keys).await?,
            state.unsubscribe_token(keys).await?,
        ) else {
            continue;
        };
        if !newsletter
            .claim_delivery(MAILER, &campaign.id, &state.id)
            .await?
        {
            continue;
        }
        // Claimed, then sent: a failure is recorded, not retried, since a
        // subscriber mailed twice is worse off than one missed.
        let link = unsubscribe_link(&storefront, &token);
        match send_campaign(cx, tenant, &subscriber.email, &copy, &link).await {
            Ok(()) => mailed += 1,
            Err(err) => {
                tracing::error!(tenant = %tenant.id, campaign = %campaign.id, subscriber = %state.id, error = %err, "newsletter not delivered");
                newsletter
                    .fail_delivery(&campaign.id, &state.id, &err.to_string())
                    .await?;
            }
        }
    }
    if owed.len() < batch as usize {
        newsletter.finish_campaign(MAILER, &campaign.id).await?;
        tracing::info!(tenant = %tenant.id, campaign = %campaign.id, "newsletter sent");
    }
    Ok(mailed)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use evento::migrator::Migrate;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx_migrator::Info;
    use timada_core::Key;
    use timada_newsletter::{CampaignDraft, RequestOutcome, RequestSubscription, SOURCE_FORM};
    use topcoat::context::CxTestBuilder;
    use topcoat::mail::{MailConfig, MemoryTransport};

    use super::*;
    use crate::auth::mail::MailSender;

    const SHOP: &str = "https://shop.example";
    const TENANT: &str = "t1";

    #[derive(Default)]
    struct MemoryKeys(Mutex<HashMap<String, Key>>);

    #[async_trait::async_trait]
    impl Keys for MemoryKeys {
        async fn put(&self, key_id: &str, key: &Key) -> anyhow::Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(key_id.to_owned(), key.clone());
            Ok(())
        }

        async fn get(&self, key_id: &str) -> anyhow::Result<Option<Key>> {
            Ok(self.0.lock().unwrap().get(key_id).cloned())
        }

        async fn delete(&self, key_id: &str) -> anyhow::Result<()> {
            self.0.lock().unwrap().remove(key_id);
            Ok(())
        }
    }

    /// One in-memory database holding both the store and the read model.
    async fn store() -> (evento::Sqlite, SqlitePool) {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
        migrator
            .add_migrations(timada_newsletter::migrations())
            .unwrap();
        migrator
            .run(&mut *conn, &evento::migrator::Plan::apply_all())
            .await
            .unwrap();
        drop(conn);
        (pool.clone().into(), pool)
    }

    async fn catch_up(store: &evento::Sqlite, pool: &SqlitePool, keys: Arc<MemoryKeys>) {
        timada_newsletter::read_model_subscription::<evento::Sqlite>(pool.clone(), TENANT, keys)
            .run_once(store)
            .await
            .unwrap();
    }

    async fn subscribe(store: &evento::Sqlite, keys: &MemoryKeys, email: &str) {
        let newsletter = timada_newsletter::Command(store.clone());
        let RequestOutcome::ToConfirm { token, .. } = newsletter
            .request_subscription(
                keys,
                "storefront",
                RequestSubscription {
                    email: email.to_owned(),
                    language: None,
                    source: SOURCE_FORM.to_owned(),
                },
                crate::auth::now(),
            )
            .await
            .unwrap()
        else {
            panic!("nothing to mail");
        };
        newsletter
            .confirm_subscription(keys, "storefront", &token, crate::auth::now())
            .await
            .unwrap();
    }

    fn mail() -> (Cx, MemoryTransport) {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(crate::shop_mail::ShopMail::new(None, false))
            .build();
        (cx, transport)
    }

    #[tokio::test]
    async fn a_campaign_goes_out_once_to_each_subscriber_in_batches() {
        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        for email in ["ada@example.com", "grace@example.com", "alan@example.com"] {
            subscribe(&store, &keys, email).await;
        }
        let newsletter = timada_newsletter::Command(store.clone());
        let campaign = newsletter
            .draft_campaign(
                "user-alice",
                CampaignDraft {
                    subject: "News from {{ shop.name }}".to_owned(),
                    html: r#"<p>New mugs.</p><a href="{{ unsubscribe_url }}">Leave</a>"#.to_owned(),
                    text: None,
                    language: None,
                },
            )
            .await
            .unwrap();
        newsletter
            .send_campaign("user-alice", &campaign)
            .await
            .unwrap();
        catch_up(&store, &pool, keys.clone()).await;
        let shop = timada_tenancy::TenantState {
            id: TENANT.into(),
            name: "Acme Tea".into(),
            ..Default::default()
        };
        let (cx, transport) = mail();

        // No storefront yet: no unsubscribe link, nothing goes out.
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            0
        );
        timada_orders::Command(store.clone())
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();

        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            2
        );
        // A sweep before the read model caught up, or another node: the
        // claims are refused, nobody is mailed twice.
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            0
        );
        catch_up(&store, &pool, keys.clone()).await;
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            1
        );
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);

        let sent = transport.sent();
        assert_eq!(sent.len(), 3);
        let mut to: Vec<String> = sent
            .iter()
            .flat_map(|mail| mail.to().iter().map(|to| to.address().to_owned()))
            .collect();
        to.sort();
        assert_eq!(
            to,
            ["ada@example.com", "alan@example.com", "grace@example.com"]
        );
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains("Subject: News from Acme Tea"));
        assert!(wire.contains("List-Unsubscribe: <https://shop.example/newsletter/unsubscribe/"));
        assert!(wire.contains("List-Unsubscribe-Post: List-Unsubscribe=One-Click"));
    }
}
