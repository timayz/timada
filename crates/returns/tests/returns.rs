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
    Command, FakeLabelProvider, IssueLabel, LabelError, LabelFile, ListReturns, ReceiveReturn,
    ReceivedLine, RefundMethod, ReplacementStatus, RequestReturn, RequestedLine, ReturnError,
    ReturnGround, ReturnPolicy, ReturnStatus, claimed_quantities, count_returns, list_returns,
    load_return, load_return_label_file, migrations, return_list_subscription,
    return_processing_subscription, returns_of_order, voucher_code,
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
        self.request_on(order_id, quantity, ReturnGround::ChangedMind)
            .await
    }

    async fn request_on(
        &self,
        order_id: &str,
        quantity: u32,
        ground: ReturnGround,
    ) -> Result<String, ReturnError> {
        self.returns()
            .request_return(RequestReturn {
                order_id: order_id.into(),
                customer_id: CUSTOMER.into(),
                lines: vec![RequestedLine {
                    product_id: PRODUCT.into(),
                    quantity,
                }],
                ground,
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
                replace: false,
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
                replace: false,
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
                replace: false,
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
        replace: false,
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
                replace: false,
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
            ground: ReturnGround::ChangedMind,
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
            ground: ReturnGround::ChangedMind,
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
        .returns_with(ReturnPolicy {
            window_days: 0,
            ..ReturnPolicy::default()
        })
        .request_return(RequestReturn {
            order_id: order_id.clone(),
            customer_id: CUSTOMER.into(),
            lines: vec![RequestedLine {
                product_id: PRODUCT.into(),
                quantity: 1,
            }],
            ground: ReturnGround::ChangedMind,
            reason: "Trop tard".into(),
        })
        .await;
    assert!(matches!(closed, Err(ReturnError::WindowClosed)));
    Ok(())
}

/// What the warehouse can still promise.
async fn available(shop: &Shop) -> anyhow::Result<u32> {
    let item = stock_item_id(PRODUCT, &StockLocation::Warehouse);
    Ok(
        timada_inventory::load_stock_availability(&shop.executor, item)
            .await?
            .map_or(0, |s| s.available),
    )
}

#[tokio::test]
async fn a_defective_unit_is_replaced_instead_of_refunded() -> anyhow::Result<()> {
    let shop = Shop::open().await?;
    let order_id = shop.order("cart-replace", None, true).await?;
    let returns = shop.returns();
    let id = shop.request(&order_id, 1).await?;
    returns.approve_return(&id).await?;

    // Nothing came back in a state to be taken: nothing to replace.
    assert!(matches!(
        returns
            .receive_return(
                &id,
                ReceiveReturn {
                    lines: received(0, false),
                    refund_method: RefundMethod::OriginalPayment,
                    replace: true,
                },
            )
            .await,
        Err(ReturnError::NothingToReplace)
    ));
    // The broken unit is not sold again; a good one leaves in its place.
    returns
        .receive_return(
            &id,
            ReceiveReturn {
                lines: received(1, false),
                refund_method: RefundMethod::OriginalPayment,
                replace: true,
            },
        )
        .await?;
    let planned = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(planned.money, Money::eur(0));
    let replacement = planned
        .replacement
        .ok_or_else(|| anyhow::anyhow!("no replacement planned"))?;
    assert_eq!(replacement.status, ReplacementStatus::Planned);
    assert_eq!(replacement.lines[0].quantity, 1);
    assert_eq!(replacement.lines[0].name, "AOC 24G4XE");
    // What the refund would have been, kept in case the stock is gone.
    assert_eq!(replacement.fallback_money, Money::eur(11_995));

    shop.process().await?;
    let done = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(done.status, ReturnStatus::Completed);
    assert_eq!((done.money, done.credit), (Money::eur(0), Money::eur(0)));
    assert_eq!(done.voucher_code, None);
    let replacement = done
        .replacement
        .ok_or_else(|| anyhow::anyhow!("replacement lost"))?;
    assert_eq!(replacement.status, ReplacementStatus::Arranged);
    let shipment_id = replacement
        .shipment_id
        .ok_or_else(|| anyhow::anyhow!("no parcel"))?;
    assert_eq!(shipment_id, timada_shipping::replacement_shipment_id(&id));
    let parcel = timada_shipping::load_shipment(&shop.executor, &shipment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("parcel missing"))?;
    assert_eq!(parcel.order_id, order_id);
    assert_eq!(parcel.replaces_return.as_deref(), Some(id.as_str()));
    assert_eq!(parcel.destination, address());
    assert_eq!(parcel.method.code, "colissimo");
    assert_eq!(
        (
            parcel.lines[0].product_id.as_str(),
            parcel.lines[0].quantity
        ),
        (PRODUCT, 1)
    );
    assert_eq!(parcel.status, timada_shipping::ShipmentStatus::Created);

    // One unit put aside, once, and not a cent given back.
    assert_eq!(shop.on_hand().await?, 10);
    assert_eq!(available(&shop).await?, 9);
    let payment = load_payment(&shop.executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, Money::eur(0));
    assert!(payment.refunds.is_empty());
    let rows = list_returns(&shop.db, &ListReturns::default()).await?;
    assert_eq!(rows[0].status, "completed");
    assert_eq!((rows[0].refunded_minor, rows[0].credited_minor), (0, 0));
    Ok(())
}

#[tokio::test]
async fn a_replacement_the_warehouse_cannot_honour_becomes_a_refund() -> anyhow::Result<()> {
    let shop = Shop::open().await?;
    let order_id = shop.order("cart-short", None, true).await?;
    let returns = shop.returns();
    let inventory = timada_inventory::Command(&shop.executor);
    let item = stock_item_id(PRODUCT, &StockLocation::Warehouse);
    let replace = |restock| ReceiveReturn {
        lines: received(1, restock),
        refund_method: RefundMethod::OriginalPayment,
        replace: true,
    };

    // The shelf is empty when the parcel is opened: the operator is told at
    // once, and refunds instead...
    inventory.reserve_stock(&item, "somebody-else", 10).await?;
    let id = shop.request(&order_id, 1).await?;
    returns.approve_return(&id).await?;
    assert!(matches!(
        returns.receive_return(&id, replace(false)).await,
        Err(ReturnError::ReplacementOutOfStock(product)) if product == PRODUCT
    ));
    let still = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(still.status, ReturnStatus::Approved);
    // ...unless the unit that came back is fit to leave again.
    returns.receive_return(&id, replace(true)).await?;
    shop.process().await?;
    let swapped = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(
        swapped.replacement.map(|r| r.status),
        Some(ReplacementStatus::Arranged)
    );
    assert_eq!(available(&shop).await?, 0);

    // The stock goes between the decision and the reservation: the return
    // falls back to the refund that was settled with it.
    inventory.release_stock(&item, "somebody-else").await?;
    let late = shop.request(&order_id, 1).await?;
    returns.approve_return(&late).await?;
    returns.receive_return(&late, replace(false)).await?;
    inventory.reserve_stock(&item, "somebody-else", 10).await?;
    shop.process().await?;

    let refunded = load_return(&shop.executor, &late)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(refunded.status, ReturnStatus::Completed);
    assert_eq!(refunded.money, Money::eur(11_995));
    let replacement = refunded
        .replacement
        .ok_or_else(|| anyhow::anyhow!("replacement lost"))?;
    assert_eq!(replacement.status, ReplacementStatus::Abandoned);
    assert_eq!(replacement.shipment_id, None);
    assert!(
        replacement
            .abandoned_reason
            .as_deref()
            .is_some_and(|reason| reason.contains(PRODUCT)),
        "{:?}",
        replacement.abandoned_reason
    );
    assert!(
        timada_shipping::load_shipment(
            &shop.executor,
            timada_shipping::replacement_shipment_id(&late)
        )
        .await?
        .is_none()
    );
    let payment = load_payment(&shop.executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, Money::eur(11_995));
    Ok(())
}

fn label_file() -> LabelFile {
    LabelFile {
        file_name: "Étiquette retour.pdf".into(),
        content_type: "application/pdf".into(),
        bytes: b"%PDF-1.4 label".to_vec(),
    }
}

#[tokio::test]
async fn a_prepaid_label_is_on_the_customer_unless_the_shop_is_at_fault() -> anyhow::Result<()> {
    let shop = Shop::open().await?;
    let order_id = shop.order("cart-label", None, true).await?;
    let returns = shop.returns_with(ReturnPolicy {
        label_fees: timada_core::PerCurrency::none().with(timada_core::Money::eur(690)),
        ..ReturnPolicy::default()
    });
    let label = |url: Option<&str>, file: Option<LabelFile>| IssueLabel {
        carrier: " Colissimo ".into(),
        tracking_number: "8R000001".into(),
        url: url.map(str::to_owned),
        file,
        waive_fee: false,
    };

    // A change of mind: accepted with its label in one go, 6,90 € for it.
    let changed = shop.request(&order_id, 1).await?;
    assert!(matches!(
        returns
            .issue_return_label(&changed, label(None, Some(label_file())))
            .await,
        Err(ReturnError::WrongStatus { .. })
    ));
    for (bad, expected) in [
        (label(None, None), "missing"),
        (label(Some("ftp://carrier.example/l.pdf"), None), "url"),
        (
            label(
                None,
                Some(LabelFile {
                    content_type: "text/html".into(),
                    ..label_file()
                }),
            ),
            "file",
        ),
    ] {
        let refused = returns.approve_return_with_label(&changed, bad).await;
        assert!(
            matches!(
                (&refused, expected),
                (Err(ReturnError::LabelMissing), "missing")
                    | (Err(ReturnError::InvalidLabelUrl), "url")
                    | (Err(ReturnError::InvalidLabelFile), "file")
            ),
            "{expected}: {refused:?}"
        );
    }
    returns
        .approve_return_with_label(&changed, label(None, Some(label_file())))
        .await?;
    let view = load_return(&shop.executor, &changed)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(view.status, ReturnStatus::Approved);
    assert_eq!(view.ground, Some(ReturnGround::ChangedMind));
    let issued = view.label.ok_or_else(|| anyhow::anyhow!("no label"))?;
    assert_eq!(issued.carrier, "Colissimo");
    assert_eq!(issued.fee, Money::eur(690));
    assert!(issued.with_approval);
    assert!(view.approved_at.is_some());
    // The name is made safe for a header; the bytes are kept as they came.
    assert_eq!(issued.file_name.as_deref(), Some("_tiquette_retour.pdf"));
    let file = load_return_label_file(&shop.db, &changed)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no label file"))?;
    assert_eq!(file.bytes, b"%PDF-1.4 label");
    assert_eq!(file.content_type, "application/pdf");
    assert!(matches!(
        returns
            .issue_return_label(&changed, label(Some("https://carrier.example/l"), None))
            .await,
        Err(ReturnError::LabelAlreadyIssued)
    ));

    // The fee comes off the refund, and the return says so.
    returns
        .receive_return(
            &changed,
            ReceiveReturn {
                lines: received(1, true),
                refund_method: RefundMethod::OriginalPayment,
                replace: false,
            },
        )
        .await?;
    shop.process().await?;
    let done = load_return(&shop.executor, &changed)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(done.money, Money::eur(11_995 - 690));
    assert_eq!(done.label_fee_deducted, Some(Money::eur(690)));
    let payment = load_payment(&shop.executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, Money::eur(11_995 - 690));

    // A defective unit: the label — a link this time, given after the
    // approval — is on the shop, and the refund is whole.
    let broken = shop
        .request_on(&order_id, 1, ReturnGround::Defective)
        .await?;
    returns.approve_return(&broken).await?;
    returns
        .issue_return_label(&broken, label(Some("https://carrier.example/l/42"), None))
        .await?;
    let view = load_return(&shop.executor, &broken)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    let issued = view.label.ok_or_else(|| anyhow::anyhow!("no label"))?;
    assert_eq!(issued.fee, Money::eur(0));
    assert!(!issued.with_approval);
    assert_eq!(issued.file_name, None);
    assert!(load_return_label_file(&shop.db, &broken).await?.is_none());
    returns
        .receive_return(
            &broken,
            ReceiveReturn {
                lines: received(1, false),
                refund_method: RefundMethod::OriginalPayment,
                replace: false,
            },
        )
        .await?;
    let done = load_return(&shop.executor, &broken)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    assert_eq!(done.money, Money::eur(11_995));
    assert_eq!(done.label_fee_deducted, None);
    Ok(())
}

#[tokio::test]
async fn a_carrier_adapter_provides_the_label_and_an_operator_may_waive_its_fee()
-> anyhow::Result<()> {
    let shop = Shop::open().await?;
    let order_id = shop.order("cart-carrier", None, true).await?;
    let returns = shop.returns_with(ReturnPolicy {
        label_fees: timada_core::PerCurrency::none().with(timada_core::Money::eur(690)),
        ..ReturnPolicy::default()
    });
    let carrier = FakeLabelProvider::default();
    let id = shop.request(&order_id, 2).await?;
    returns.approve_return(&id).await?;

    // The carrier says no: nothing is recorded, the operator may ask again.
    carrier.answer(Err(LabelError::Refused("address not served".into())));
    assert!(matches!(
        returns.provide_return_label(&id, &carrier, true).await,
        Err(ReturnError::LabelProvider(LabelError::Refused(_)))
    ));
    returns.provide_return_label(&id, &carrier, true).await?;
    let asked = carrier.asked();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].sender, address());
    assert_eq!(asked[1].units, 2);

    let view = load_return(&shop.executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("return missing"))?;
    let issued = view.label.ok_or_else(|| anyhow::anyhow!("no label"))?;
    assert!(issued.tracking_number.starts_with("8R"), "{issued:?}");
    // A change of mind, but the operator made a gesture.
    assert_eq!(issued.fee, Money::eur(0));
    let file = load_return_label_file(&shop.db, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no label file"))?;
    assert!(file.bytes.starts_with(b"%PDF-"));
    // One label per return: the carrier is not asked again.
    assert!(matches!(
        returns.provide_return_label(&id, &carrier, false).await,
        Err(ReturnError::LabelAlreadyIssued)
    ));
    assert_eq!(carrier.asked().len(), 2);
    Ok(())
}

#[test]
fn a_label_fee_is_said_per_currency_and_free_where_nothing_is_said() {
    let policy = ReturnPolicy {
        label_fees: [Money::eur(690), Money::new(590, "GBP")]
            .into_iter()
            .collect(),
        ..ReturnPolicy::default()
    };
    let changed = Some(ReturnGround::ChangedMind);
    assert_eq!(policy.label_fee(changed, false, "EUR"), Money::eur(690));
    assert_eq!(
        policy.label_fee(changed, false, "GBP"),
        Money::new(590, "GBP")
    );
    // Never 6,90 of whatever the order was paid in.
    assert_eq!(
        policy.label_fee(changed, false, "CHF"),
        Money::new(0, "CHF")
    );
    assert_eq!(
        policy.label_fee(Some(ReturnGround::Damaged), false, "GBP"),
        Money::new(0, "GBP")
    );
    assert_eq!(policy.label_fee(changed, true, "EUR"), Money::eur(0));
    // A return older than grounds counts as a change of mind.
    assert_eq!(policy.label_fee(None, false, "EUR"), Money::eur(690));
    assert_eq!(
        ReturnPolicy::default().label_fee(changed, false, "EUR"),
        Money::eur(0)
    );
}
