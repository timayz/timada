//! A provider for the crate's own tests: it knows the payments it is told
//! of and nothing else, and does nothing but say where they stand.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{
    Cancellation, CaptureAnswer, METADATA_CART, METADATA_TENANT, NewPayment, OpenPayment,
    PaymentGateway, PaymentState, PaymentStatus, RefundRecord, Refused, ReleaseAnswer,
};

#[derive(Default)]
pub struct FakeGateway {
    known: Mutex<HashMap<String, PaymentStatus>>,
    /// Answers every question with this error instead, as a provider that
    /// cannot be reached.
    down: Mutex<Option<String>>,
}

impl FakeGateway {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// A payment the checkout opened for `cart_id` of `tenant_id`, and the
    /// shopper confirmed: `amount_cents` is held.
    pub fn hold(&self, payment_id: &str, tenant_id: &str, cart_id: &str, amount_cents: u64) {
        self.known.lock().unwrap().insert(
            payment_id.to_owned(),
            PaymentStatus {
                payment_id: payment_id.to_owned(),
                state: PaymentState::Authorized,
                amount_capturable: amount_cents,
                amount_received: 0,
                currency: "eur".to_owned(),
                capture_before: None,
                lapsed: false,
                metadata: HashMap::from([
                    (METADATA_TENANT.to_owned(), tenant_id.to_owned()),
                    (METADATA_CART.to_owned(), cart_id.to_owned()),
                ]),
            },
        );
    }

    /// Moves a known payment to `state`.
    pub fn set_state(&self, payment_id: &str, state: PaymentState) {
        let mut known = self.known.lock().unwrap();
        let status = known.get_mut(payment_id).expect("a payment told of");
        status.state = state;
        if state != PaymentState::Authorized {
            status.amount_capturable = 0;
        }
    }

    /// Answers everything with `reason` from now on; `None` answers again.
    pub fn set_down(&self, reason: Option<&str>) {
        *self.down.lock().unwrap() = reason.map(str::to_owned);
    }

    fn reachable(&self) -> anyhow::Result<()> {
        match self.down.lock().unwrap().as_deref() {
            Some(reason) => Err(anyhow::anyhow!("{reason}")),
            None => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl PaymentGateway for FakeGateway {
    async fn check_key(&self, _secret_key: &str) -> anyhow::Result<()> {
        self.reachable()
    }

    async fn open_payment(
        &self,
        _secret_key: &str,
        _payment: NewPayment,
    ) -> anyhow::Result<OpenPayment> {
        anyhow::bail!("the fake provider opens no payment")
    }

    async fn payment_status(
        &self,
        _secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<PaymentStatus> {
        self.reachable()?;
        self.known
            .lock()
            .unwrap()
            .get(payment_id)
            .cloned()
            .ok_or_else(|| {
                anyhow::Error::new(Refused {
                    message: format!("No such payment_intent: '{payment_id}'"),
                    code: Some("resource_missing".into()),
                })
            })
    }

    async fn refund(
        &self,
        _secret_key: &str,
        _payment_id: &str,
        _amount_cents: u64,
        _idempotency_key: &str,
    ) -> anyhow::Result<String> {
        anyhow::bail!("the fake provider refunds nothing")
    }

    async fn refunds_of(
        &self,
        _secret_key: &str,
        _payment_id: &str,
    ) -> anyhow::Result<Vec<RefundRecord>> {
        Ok(Vec::new())
    }

    async fn cancel_payment(
        &self,
        _secret_key: &str,
        _payment_id: &str,
    ) -> anyhow::Result<Cancellation> {
        anyhow::bail!("the fake provider cancels nothing")
    }

    async fn capture_payment(
        &self,
        _secret_key: &str,
        _payment_id: &str,
    ) -> anyhow::Result<CaptureAnswer> {
        anyhow::bail!("the fake provider takes nothing")
    }

    async fn release_payment(
        &self,
        _secret_key: &str,
        _payment_id: &str,
    ) -> anyhow::Result<ReleaseAnswer> {
        anyhow::bail!("the fake provider releases nothing")
    }
}
