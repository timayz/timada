//! A return from request to completion — restock, refund to the original
//! payment, store credit — and everything that guards it: who may ask, until
//! when, for how many units, and what a retry may never do twice.

use timada_core::{Address, Money};
use timada_inventory::{RegisterStockItem, StockLocation, stock_item_id};
use timada_order::{
    DeliveryChoice, OrderDiscount, OrderLine, PaymentMode, PlaceOrder, PromoKind, Seller,
};
use timada_payment::{PaymentMethod, RequestPayment, load_payment, payment_id};
use timada_promotion::{VoucherKind, load_voucher_balance, voucher_id};
use timada_returns::{
    Command, ListReturns, ReceiveReturn, ReceivedLine, RefundMethod, RequestReturn, RequestedLine,
    ReturnError, ReturnPolicy, ReturnStatus, claimed_quantities, count_returns, list_returns,
    load_return, migrations, return_list_subscription, return_processing_subscription,
    returns_of_order, voucher_code,
};

const PRODUCT: &str = "aoc-24g4xe";
const CUSTOMER: &str = "customer-1";

fn address() -> Address {
    Address {
        first_name: "Ada".into(),
        last_name: "Lovelace".into(),
        line1: "12 rue des Machines".into(),
        postal_code: "31000".into(),
        city: "Toulouse".into(),
        country_code: "FR".into(),
        ..Address::default()
    }
}

struct Shop {
    executor: evento::Sqlite,
    db: sqlx::SqlitePool,
}

impl Shop {
    async fn open() -> anyhow::Result<Self> {
        let mut all = migrations();
        all.extend(timada_order::migrations());
        all.extend(timada_promotion::migrations());
        all.extend(timada_payment::migrations());
        let (executor, db) = timada_core::testing::memory_executor(all).await?;
        let inventory = timada_inventory::Command(&executor);
        let item = inventory
            .register_stock_item(RegisterStockItem {
                product_id: PRODUCT.into(),
                location: StockLocation::Warehouse,
            })
            .await?;
        inventory.receive_stock(&item, 10).await?;
        Ok(Self { executor, db })
    }

    fn returns(&self) -> Command<'_, evento::Sqlite> {
        self.returns_with(ReturnPolicy::default())
    }

    fn returns_with(&self, policy: ReturnPolicy) -> Command<'_, evento::Sqlite> {
        Command {
            executor: &self.executor,
            db: self.db.clone(),
            policy,
        }
    }

    /// Two units at 119,95 € plus 5,90 € of shipping, paid by card. With
    /// `ship`, the order is also marked shipped.
    async fn order(
        &self,
        cart: &str,
        discount: Option<OrderDiscount>,
        ship: bool,
    ) -> anyhow::Result<String> {
        let orders = timada_order::Command(&self.executor);
        let taken_off = discount.as_ref().map_or(0, |d| d.amount.minor);
        let order_id = orders
            .place_order(PlaceOrder {
                cart_id: cart.into(),
                customer_id: CUSTOMER.into(),
                seller: Seller::Ldlc,
                lines: vec![OrderLine {
                    product_id: PRODUCT.into(),
                    name: "AOC 24G4XE".into(),
                    quantity: 2,
                    unit_price: Money::eur(11_995),
                    warranty_months: 36,
                }],
                delivery_address: address(),
                billing_address: address(),
                delivery: DeliveryChoice {
                    method_code: "colissimo".into(),
                    pickup_store_id: None,
                },
                payment_mode: PaymentMode::Card,
                shipping_fee: Money::eur(590),
                handling_fee: Money::eur(0),
                promo_code: discount.as_ref().map(|d| d.code.clone()),
                discount,
                order_number: None,
                tax: None,
                business: None,
            })
            .await?;
        let payments = timada_payment::Command(&self.executor);
        let payment = payments
            .request_payment(RequestPayment {
                order_id: order_id.clone(),
                amount: Money::eur(23_990 + 590 - taken_off),
                method: PaymentMethod::Card,
            })
            .await?;
        payments.capture_payment(&payment, "psp-1".into()).await?;
        orders.mark_paid(&order_id, &payment).await?;
        if ship {
            orders
                .mark_shipped(&order_id, "shipment-1", "Colissimo".into(), "XY123".into())
                .await?;
        }
        Ok(order_id)
    }

    async fn request(&self, order_id: &str, quantity: u32) -> Result<String, ReturnError> {
        self.returns()
            .request_return(RequestReturn {
                order_id: order_id.into(),
                customer_id: CUSTOMER.into(),
                lines: vec![RequestedLine {
                    product_id: PRODUCT.into(),
                    quantity,
                }],
                reason: "Ne convient pas".into(),
            })
            .await
    }

    /// Runs the process manager and the list read model, twice over: a
    /// redelivery must change nothing.
    async fn process(&self) -> anyhow::Result<()> {
        for _ in 0..2 {
            return_processing_subscription()
                .data(self.db.clone())
                .run_once(&self.executor)
                .await?;
            return_list_subscription()
                .data(self.db.clone())
                .run_once(&self.executor)
                .await?;
        }
        // The refunds the process manager asked for go back at once: no
        // payment provider here.
        timada_payment::refund_execution_subscription()
            .data(self.db.clone())
            .run_once(&self.executor)
            .await?;
        timada_payment::execute_pending_refunds(
            &self.executor,
            &self.db,
            &timada_payment::ManualProvider,
            &timada_payment::RefundPolicy::without_delays(),
        )
        .await?;
        Ok(())
    }

    async fn on_hand(&self) -> anyhow::Result<u32> {
        let item = stock_item_id(PRODUCT, &StockLocation::Warehouse);
        Ok(
            timada_inventory::load_stock_availability(&self.executor, item)
                .await?
                .map_or(0, |s| s.on_hand),
        )
    }
}

fn received(accepted: u32, restock: bool) -> Vec<ReceivedLine> {
    vec![ReceivedLine {
        product_id: PRODUCT.into(),
        accepted,
        restock,
    }]
}

#[tokio::test]
async fn a_return_is_restocked_and_refunded_to_the_original_payment() -> anyhow::Result<()> {
    let shop = Shop::open().await?;
    let order_id = shop.order("cart-1", None, true).await?;
    let returns = shop.returns();

    let id = shop.request(&order_id, 1).await?;
    let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
    let view = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(view.rma_number, format!("R{year}-000001"));
    assert_eq!(view.status, ReturnStatus::Requested);
    assert_eq!(view.lines[0].name, "AOC 24G4XE");
    assert_eq!(view.lines[0].unit_price, Money::eur(11_995));

    // Not before an operator approved it.
    let early = returns
        .receive_return(
            &id,
            ReceiveReturn {
                lines: received(1, true),
                refund_method: RefundMethod::OriginalPayment,
            },
        )
        .await;
    assert!(matches!(early, Err(ReturnError::WrongStatus { .. })));
    returns.approve_return(&id).await?;
    returns.approve_return(&id).await?;

    let too_many = returns
        .receive_return(
            &id,
            ReceiveReturn {
                lines: received(2, true),
                refund_method: RefundMethod::OriginalPayment,
            },
        )
        .await;
    assert!(matches!(
        too_many,
        Err(ReturnError::AcceptedExceedsRequested(_))
    ));
    returns
        .receive_return(
            &id,
            ReceiveReturn {
                lines: received(1, true),
                refund_method: RefundMethod::OriginalPayment,
            },
        )
        .await?;
    shop.process().await?;

    // The goods at the price paid — not the shipping fee — once.
    let done = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(done.status, ReturnStatus::Completed);
    assert_eq!(done.money, Money::eur(11_995));
    assert_eq!(done.credit, Money::eur(0));
    assert_eq!(done.voucher_code, None);
    let payment = load_payment(&shop.executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, Money::eur(11_995));
    assert_eq!(shop.on_hand().await?, 11);

    // One unit is left to return, not two.
    let greedy = shop.request(&order_id, 2).await;
    assert!(matches!(
        greedy,
        Err(ReturnError::QuantityExceeded { returnable: 1, .. })
    ));
    assert_eq!(
        claimed_quantities(&shop.db, &order_id).await?,
        vec![(PRODUCT.to_owned(), 1)]
    );

    let rows = returns_of_order(&shop.db, &order_id).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "completed");
    assert_eq!((rows[0].units, rows[0].refunded_minor), (1, 11_995));
    let completed = ListReturns {
        status: Some(ReturnStatus::Completed),
        ..ListReturns::default()
    };
    assert_eq!(list_returns(&shop.db, &completed).await?.len(), 1);
    assert_eq!(
        count_returns(&shop.db, Some(ReturnStatus::Requested)).await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_voucher_comes_back_as_credit_and_a_promo_code_does_not_come_back() -> anyhow::Result<()>
{
    let shop = Shop::open().await?;
    let returns = shop.returns();
    let receive_all = |method| ReceiveReturn {
        lines: received(2, false),
        refund_method: method,
    };

    // 50 € of a gift voucher on 239,90 € of goods: that part was not money.
    let with_voucher = shop
        .order(
            "cart-voucher",
            Some(OrderDiscount {
                code: "GIFT50".into(),
                kind: PromoKind::Voucher,
                amount: Money::eur(5_000),
            }),
            true,
        )
        .await?;
    let id = shop.request(&with_voucher, 2).await?;
    returns.approve_return(&id).await?;
    returns
        .receive_return(&id, receive_all(RefundMethod::OriginalPayment))
        .await?;
    shop.process().await?;
    let done = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(done.money, Money::eur(18_990));
    assert_eq!(done.credit, Money::eur(5_000));
    let code = voucher_code(&done.rma_number);
    assert_eq!(done.voucher_code.as_deref(), Some(code.as_str()));
    let voucher = load_voucher_balance(&shop.executor, voucher_id(&code))
        .await?
        .ok_or_else(|| anyhow::anyhow!("store credit not issued"))?;
    assert_eq!(voucher.remaining, Money::eur(5_000));
    assert_eq!(voucher.customer_id.as_deref(), Some(CUSTOMER));
    assert!(matches!(voucher.kind, VoucherKind::CreditNote { .. }));
    // Damaged goods are taken back but not put on the shelf again.
    assert_eq!(shop.on_hand().await?, 10);

    // 10 % off with a promo code: a price reduction, refunded at the price
    // paid — here as store credit, the operator's choice.
    let with_promo = shop
        .order(
            "cart-promo",
            Some(OrderDiscount {
                code: "WELCOME10".into(),
                kind: PromoKind::Discount,
                amount: Money::eur(2_399),
            }),
            true,
        )
        .await?;
    let id = shop.request(&with_promo, 2).await?;
    returns.approve_return(&id).await?;
    returns
        .receive_return(&id, receive_all(RefundMethod::StoreCredit))
        .await?;
    shop.process().await?;
    let done = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(done.money, Money::eur(0));
    assert_eq!(done.credit, Money::eur(23_990 - 2_399));
    let payment = load_payment(&shop.executor, payment_id(&with_promo))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, Money::eur(0));
    Ok(())
}

#[tokio::test]
async fn what_the_payment_cannot_give_back_becomes_store_credit() -> anyhow::Result<()> {
    let shop = Shop::open().await?;
    let order_id = shop.order("cart-1", None, true).await?;
    // A goodwill gesture already took 200 € of the 245,80 € captured.
    timada_payment::Command(&shop.executor)
        .refund_payment(payment_id(&order_id), Money::eur(20_000), "goodwill".into())
        .await?;

    let id = shop.request(&order_id, 2).await?;
    let returns = shop.returns();
    returns.approve_return(&id).await?;
    returns
        .receive_return(
            &id,
            ReceiveReturn {
                lines: received(2, true),
                refund_method: RefundMethod::OriginalPayment,
            },
        )
        .await?;
    shop.process().await?;

    let done = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(done.money, Money::eur(4_580));
    assert_eq!(done.credit, Money::eur(23_990 - 4_580));
    let payment = load_payment(&shop.executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, payment.amount);
    Ok(())
}

#[tokio::test]
async fn who_may_ask_until_when_and_what_frees_the_units() -> anyhow::Result<()> {
    let shop = Shop::open().await?;
    let returns = shop.returns();

    let waiting = shop.order("cart-not-shipped", None, false).await?;
    assert!(matches!(
        shop.request(&waiting, 1).await,
        Err(ReturnError::OrderNotShipped)
    ));

    let order_id = shop.order("cart-1", None, true).await?;
    let stranger = returns
        .request_return(RequestReturn {
            order_id: order_id.clone(),
            customer_id: "someone-else".into(),
            lines: vec![RequestedLine {
                product_id: PRODUCT.into(),
                quantity: 1,
            }],
            reason: "Ne convient pas".into(),
        })
        .await;
    assert!(matches!(stranger, Err(ReturnError::OrderNotFound)));
    let unknown = returns
        .request_return(RequestReturn {
            order_id: order_id.clone(),
            customer_id: CUSTOMER.into(),
            lines: vec![RequestedLine {
                product_id: "not-in-the-order".into(),
                quantity: 1,
            }],
            reason: "Ne convient pas".into(),
        })
        .await;
    assert!(matches!(unknown, Err(ReturnError::UnknownLine(_))));
    assert!(matches!(
        shop.request(&order_id, 0).await,
        Err(ReturnError::NoLines)
    ));

    // A refused return and a cancelled one give their units back.
    let refused = shop.request(&order_id, 2).await?;
    assert!(matches!(
        shop.request(&order_id, 1).await,
        Err(ReturnError::QuantityExceeded { returnable: 0, .. })
    ));
    returns.refuse_return(&refused, "Hors conditions").await?;
    let cancelled = shop.request(&order_id, 2).await?;
    assert!(matches!(
        returns.cancel_return(&cancelled, "someone-else").await,
        Err(ReturnError::ReturnNotFound)
    ));
    returns.approve_return(&cancelled).await?;
    returns.cancel_return(&cancelled, CUSTOMER).await?;
    returns.cancel_return(&cancelled, CUSTOMER).await?;
    assert!(claimed_quantities(&shop.db, &order_id).await?.is_empty());
    let view = load_return(&shop.executor, &refused)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(view.status, ReturnStatus::Refused);
    assert_eq!(view.refused_reason.as_deref(), Some("Hors conditions"));

    // Past the window nothing can be asked for any more.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let closed = shop
        .returns_with(ReturnPolicy { window_days: 0 })
        .request_return(RequestReturn {
            order_id: order_id.clone(),
            customer_id: CUSTOMER.into(),
            lines: vec![RequestedLine {
                product_id: PRODUCT.into(),
                quantity: 1,
            }],
            reason: "Trop tard".into(),
        })
        .await;
    assert!(matches!(closed, Err(ReturnError::WindowClosed)));
    Ok(())
}
