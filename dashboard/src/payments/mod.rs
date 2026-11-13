//! Card payments, on each tenant's own Stripe account.
//!
//! A storefront asks to pay (`Cart::CheckoutRequested`); [`subscription`]
//! has the cart's supplied goods read again ([`SuppliedGoods`]), prices the
//! cart from the store, opens a payment with Stripe under the tenant's secret
//! key and records it (`Cart::PaymentRequested`) for the storefront's card
//! form. The payment only holds the money on the shopper's card: Stripe
//! tells the webhook (`app/stripe`) when the shopper confirmed it, and the
//! order then exists, its money held. It is taken ([`settle`]) at once for
//! goods the shop holds ([`capture`]), or once the supplier has the goods.
//! Cancelling an order gives the hold back, or the money once taken,
//! through Stripe first. A card form left open past the time its goods were
//! held for is closed by [`lapsed`]'s sweep before it can be confirmed;
//! what an order asked of Stripe and got no answer to is settled by
//! [`pending`]'s, and a request to pay its claimant never answered is
//! refused by [`unanswered`]'s.
//!
//! The tenant's Stripe secrets are recorded sealed ([`Sealer`]) under the
//! operator's `secrets.key`; without a
//! `[payments]` section no tenant takes payments.

mod capture;
#[cfg(test)]
pub mod fake;
mod gateway;
mod goods;
mod lapsed;
mod pending;
pub mod settle;
mod setup;
pub mod signature;
mod subscription;
mod unanswered;
pub mod webhook;

use std::sync::Arc;

pub use capture::*;
pub use gateway::*;
pub use goods::*;
pub use lapsed::*;
pub use pending::*;
pub use setup::*;
pub use subscription::*;
pub use unanswered::*;

use timada_orders::{OrderState, PROVIDER_STRIPE};
use timada_tenancy::{CompanyState, TenantState};

use crate::seal::Sealer;

/// Who the dashboard's own payment events say did them.
pub const DASHBOARD_ACTOR: &str = "timada-dashboard";

/// The metadata every payment is opened with, read back from the provider:
/// the tenant and the cart it was opened for.
pub const METADATA_TENANT: &str = "tenant_id";
pub const METADATA_CART: &str = "cart_id";

/// What the provider says of the payment an order says it was placed with.
/// A `Placed` the checkout wrote names a payment the provider opened for
/// the tenant's cart; one a `store:write` key forged in the tenant's store
/// names none the provider knows as this cart's, so this tells the orders
/// the checkout placed from the rest (timada #431).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderPayment {
    /// The provider holds the money, or took it.
    Held,
    /// The provider opened it for this cart but holds nothing any more: the
    /// hold went back, or lapsed.
    Gone,
    /// The provider knows no such payment of the tenant's account, or opened
    /// it for another cart or tenant; or the tenant has no account to ask.
    Unknown,
}

/// A tenant's Stripe account, opened.
#[derive(Clone)]
pub struct Account {
    pub publishable_key: String,
    pub secret_key: String,
    pub webhook_secret: String,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("publishable_key", &self.publishable_key)
            .finish_non_exhaustive()
    }
}

/// The payments service: the provider, and the key to the tenants' secrets.
/// It holds no store: a tenant and its company are read from the store its
/// caller reads, a page's through the cluster, the background's from this
/// node's file (#533).
pub struct Payments {
    sealer: Option<Sealer>,
    pub gateway: Arc<dyn PaymentGateway>,
}

impl Payments {
    /// `sealer`: `None` turns payments off for every tenant (see
    /// [`payments_sealer`](crate::seal::payments_sealer)).
    pub fn new(sealer: Option<Sealer>, gateway: Arc<dyn PaymentGateway>) -> Self {
        Self { sealer, gateway }
    }

    /// Whether the operator enabled payments on this node.
    pub fn enabled(&self) -> bool {
        self.sealer.is_some()
    }

    fn sealer(&self) -> anyhow::Result<&Sealer> {
        self.sealer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("payments are not enabled (no [payments] section)"))
    }

    /// Seals a secret of `tenant_id`'s account: it opens for that tenant
    /// alone.
    pub fn seal(&self, tenant_id: &str, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.sealer()?.seal_for(tenant_id, plain)
    }

    /// The account new payments open with: none once the tenant stopped
    /// taking payments, whose keys only settle what is held or owed.
    pub fn checkout_account_of(&self, tenant: &TenantState) -> anyhow::Result<Option<Account>> {
        let stopped = tenant
            .payments
            .as_ref()
            .is_some_and(timada_tenancy::PaymentSettings::is_stopped);
        if stopped {
            return Ok(None);
        }
        self.account_of(tenant)
    }

    /// Whether `secret_key` reads the payment `payment_id`, one the tenant
    /// still holds or owes: whether it opens the account that payment was
    /// made through. Another account's key is told there is no such payment.
    pub async fn reads_payment(&self, secret_key: &str, payment_id: &str) -> anyhow::Result<bool> {
        match self.gateway.payment_status(secret_key, payment_id).await {
            Ok(_) => Ok(true),
            Err(err) if err.downcast_ref::<Refused>().is_some() => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// What the provider says of the payment `order` says it was placed
    /// with, under `tenant`'s account (see [`OrderPayment`]). An error is a
    /// provider that could not be reached: asked again later, it answers.
    pub async fn order_payment(
        &self,
        tenant: &TenantState,
        order: &OrderState,
    ) -> anyhow::Result<OrderPayment> {
        let payment = &order.payment;
        if payment.provider != PROVIDER_STRIPE || payment.payment_id.is_empty() {
            return Ok(OrderPayment::Unknown);
        }
        let Some(account) = self.account_of(tenant)? else {
            return Ok(OrderPayment::Unknown);
        };
        let status = match self
            .gateway
            .payment_status(&account.secret_key, &payment.payment_id)
            .await
        {
            Ok(status) => status,
            Err(err) if err.downcast_ref::<Refused>().is_some() => {
                return Ok(OrderPayment::Unknown);
            }
            Err(err) => return Err(err),
        };
        let opened_for = |key: &str| status.metadata.get(key).map(String::as_str);
        if opened_for(METADATA_TENANT) != Some(tenant.id.as_str())
            || opened_for(METADATA_CART) != Some(order.cart_id.as_str())
        {
            return Ok(OrderPayment::Unknown);
        }
        Ok(match status.state {
            PaymentState::Authorized | PaymentState::Captured => OrderPayment::Held,
            PaymentState::Open | PaymentState::Processing | PaymentState::Cancelled => {
                OrderPayment::Gone
            }
        })
    }

    /// The tenant's Stripe account, opened; `None` when it set up none or
    /// payments are off here. A tenant that stopped taking payments keeps
    /// it until the merchant forgets it, to settle what is held or owed.
    pub fn account_of(&self, tenant: &TenantState) -> anyhow::Result<Option<Account>> {
        let (Some(settings), Some(sealer)) = (&tenant.payments, &self.sealer) else {
            return Ok(None);
        };
        Ok(Some(Account {
            publishable_key: settings.publishable_key.clone(),
            secret_key: sealer.open_for(&tenant.id, &settings.secret_key_sealed)?,
            webhook_secret: sealer.open_for(&tenant.id, &settings.webhook_secret_sealed)?,
        }))
    }

    /// The company a tenant sells as, once it has an identity, read from
    /// `store`.
    pub async fn company_of<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant: &TenantState,
    ) -> anyhow::Result<Option<CompanyState>> {
        timada_tenancy::Command(store.clone())
            .load_company_of(tenant)
            .await
    }

    /// The tenant read from the platform's streams in `store`, unless it is
    /// suspended.
    pub async fn active_tenant<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant_id: &str,
    ) -> anyhow::Result<Option<TenantState>> {
        let tenant = timada_tenancy::Command(store.clone())
            .load_tenant(tenant_id)
            .await?;
        Ok(tenant.filter(|tenant| !tenant.is_suspended()))
    }
}

#[cfg(test)]
mod tests {
    use evento::Evento;
    use timada_orders::{OrderState, PROVIDER_STRIPE, Payment};
    use timada_tenancy::{ConfigurePayments, CreateTenant};

    use super::fake::FakeGateway;
    use super::*;

    /// A tenant with its account, an order placed for `payment`; what the
    /// provider is asked about it.
    struct Checked {
        provider: Arc<FakeGateway>,
        payments: Payments,
        tenant: TenantState,
        store: Evento,
    }

    impl Checked {
        async fn new() -> Self {
            let store = Evento::new(evento::Turso::in_memory_concurrent().await.unwrap());
            let provider = FakeGateway::new();
            let payments = Payments::new(Some(Sealer::new("payments", "secret")), provider.clone());
            let tenancy = timada_tenancy::Command(store.clone());
            let tenant = tenancy
                .create_tenant(CreateTenant {
                    name: "Acme".into(),
                    owner_user_id: "alice".into(),
                })
                .await
                .unwrap();
            tenancy
                .configure_payments(
                    "alice",
                    &tenant.id,
                    ConfigurePayments {
                        provider: PROVIDER_STRIPE.into(),
                        publishable_key: "pk_test_123".into(),
                        secret_key_sealed: payments.seal(&tenant.id, "sk_test_good").unwrap(),
                        webhook_secret_sealed: payments.seal(&tenant.id, "whsec_test").unwrap(),
                    },
                )
                .await
                .unwrap();
            let tenant = tenancy.load_tenant(&tenant.id).await.unwrap().unwrap();
            Self {
                provider,
                payments,
                tenant,
                store,
            }
        }

        /// An order of `cart_id`, as placed for `payment`.
        async fn order(&self, cart_id: &str, payment: Payment) -> OrderState {
            let order_id = format!("order-of-{cart_id}");
            evento::append(&order_id)
                .tenant(&self.tenant.id)
                .original_version(0)
                .event(&timada_orders::aggregator::Placed {
                    reference: "K7Q4-MZP2".to_owned(),
                    cart_id: cart_id.to_owned(),
                    email: "ada@example.com".to_owned(),
                    shipping: Default::default(),
                    note: None,
                    lines: Vec::new(),
                    currency: "EUR".to_owned(),
                    total_cents: 862,
                    payment,
                    supplier: None,
                    placed_by: "storefront".to_owned(),
                })
                .commit(&self.store)
                .await
                .unwrap();
            timada_orders::Command::new(self.store.clone(), &self.tenant.id)
                .load_order(&order_id)
                .await
                .unwrap()
                .unwrap()
        }

        async fn of(&self, order: &OrderState) -> OrderPayment {
            self.payments
                .order_payment(&self.tenant, order)
                .await
                .unwrap()
        }
    }

    fn payment(payment_id: &str) -> Payment {
        Payment {
            provider: PROVIDER_STRIPE.to_owned(),
            payment_id: payment_id.to_owned(),
            amount_cents: 862,
            currency: "EUR".to_owned(),
            capture_before: None,
        }
    }

    #[tokio::test]
    async fn the_provider_tells_the_orders_the_checkout_placed() {
        let checked = Checked::new().await;
        let tenant_id = checked.tenant.id.clone();

        // The checkout's order: the provider opened its payment for its cart.
        checked.provider.hold("pi_real", &tenant_id, "cart-1", 862);
        let real = checked.order("cart-1", payment("pi_real")).await;
        assert_eq!(checked.of(&real).await, OrderPayment::Held);
        checked
            .provider
            .set_state("pi_real", PaymentState::Captured);
        assert_eq!(checked.of(&real).await, OrderPayment::Held);
        checked
            .provider
            .set_state("pi_real", PaymentState::Cancelled);
        assert_eq!(checked.of(&real).await, OrderPayment::Gone);

        // A forged one: a payment the provider never opened, or none.
        let forged = checked.order("cart-2", payment("pi_forged")).await;
        assert_eq!(checked.of(&forged).await, OrderPayment::Unknown);
        let blank = checked.order("cart-3", Payment::default()).await;
        assert_eq!(checked.of(&blank).await, OrderPayment::Unknown);

        // One borrowing another cart's payment, or another tenant's.
        checked
            .provider
            .hold("pi_other_cart", &tenant_id, "cart-9", 862);
        let borrowed = checked.order("cart-4", payment("pi_other_cart")).await;
        assert_eq!(checked.of(&borrowed).await, OrderPayment::Unknown);
        checked
            .provider
            .hold("pi_other_shop", "zeta", "cart-5", 862);
        let borrowed = checked.order("cart-5", payment("pi_other_shop")).await;
        assert_eq!(checked.of(&borrowed).await, OrderPayment::Unknown);
    }

    #[tokio::test]
    async fn a_provider_that_does_not_answer_is_an_error_to_ask_again() {
        let checked = Checked::new().await;
        checked
            .provider
            .hold("pi_real", &checked.tenant.id, "cart-1", 862);
        let real = checked.order("cart-1", payment("pi_real")).await;
        checked.provider.set_down(Some("connection reset"));
        let err = checked
            .payments
            .order_payment(&checked.tenant, &real)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("connection reset"), "{err}");
    }

    #[tokio::test]
    async fn without_an_account_no_order_is_known() {
        let checked = Checked::new().await;
        checked
            .provider
            .hold("pi_real", &checked.tenant.id, "cart-1", 862);
        let real = checked.order("cart-1", payment("pi_real")).await;
        let mut no_account = checked.tenant.clone();
        no_account.payments = None;
        assert_eq!(
            checked
                .payments
                .order_payment(&no_account, &real)
                .await
                .unwrap(),
            OrderPayment::Unknown
        );
        let off = Payments::new(None, checked.provider.clone());
        assert_eq!(
            off.order_payment(&checked.tenant, &real).await.unwrap(),
            OrderPayment::Unknown
        );
    }
}
