use timada_cart::{
    AddLine, CartError, CartStatus, Checkout, Command, DeliveryChoice, PaymentMode,
    load_cart_details,
};
use timada_core::{Address, Money};

fn aoc_monitor() -> AddLine {
    AddLine {
        product_id: "product-aoc-24g4xe".into(),
        name: "AOC 23.8\" LED - Q24G4RE".into(),
        quantity: 1,
        unit_price: Money::eur(12_496),
        warranty_months: 60,
    }
}

fn headset() -> AddLine {
    AddLine {
        product_id: "product-akg-k361".into(),
        name: "AKG K361".into(),
        quantity: 2,
        unit_price: Money::eur(7_416),
        warranty_months: 24,
    }
}

fn delivery_address() -> Address {
    Address {
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "La roso".into(),
        postal_code: "97330".into(),
        city: "Saint ane".into(),
        country_code: "GP".into(),
        ..Address::default()
    }
}

fn billing_address() -> Address {
    Address {
        line1: "121, Avenue Tolosane".into(),
        line2: Some("Apt A21".into()),
        postal_code: "31520".into(),
        city: "Ramonville-Saint-Agne".into(),
        country_code: "FR".into(),
        ..delivery_address()
    }
}

fn checkout_in_3x(customer_id: Option<&str>) -> Checkout {
    Checkout {
        customer_id: customer_id.map(str::to_owned),
        delivery_address: delivery_address(),
        billing_address: billing_address(),
        delivery: DeliveryChoice {
            method_code: "chronopost-dom".into(),
            pickup_store_id: None,
        },
        payment_mode: PaymentMode::Installments { count: 3 },
    }
}

#[tokio::test]
async fn cart_details_reflect_commands_through_checkout() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(executor.clone());

    let id = cmd.open_cart(None).await?;
    cmd.add_line(&id, aoc_monitor()).await?;
    cmd.add_line(&id, headset()).await?;
    cmd.change_line_quantity(&id, "product-aoc-24g4xe".into(), 2)
        .await?;
    cmd.remove_line(&id, "product-akg-k361".into()).await?;

    let view = load_cart_details(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("cart missing"))?;
    assert_eq!(view.lines.len(), 1);
    assert_eq!(view.lines[0].quantity, 2);
    assert_eq!(view.subtotal, Money::eur(24_992));
    assert_eq!(view.status, CartStatus::Open);
    assert_eq!(view.customer_id, None);

    cmd.apply_promo_code(&id, " welcome10 ".into()).await?;
    cmd.checkout(&id, checkout_in_3x(Some("customer-1")))
        .await?;

    let view = load_cart_details(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("cart missing"))?;
    assert_eq!(view.promo_code.as_deref(), Some("WELCOME10"));
    assert_eq!(view.status, CartStatus::CheckedOut);
    assert_eq!(view.customer_id.as_deref(), Some("customer-1"));

    let frozen = cmd.add_line(&id, headset()).await;
    assert!(matches!(frozen, Err(CartError::CartAlreadyCheckedOut)));

    Ok(())
}

#[tokio::test]
async fn checkout_guards() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(executor.clone());

    let id = cmd.open_cart(None).await?;
    let empty = cmd.checkout(&id, checkout_in_3x(Some("customer-1"))).await;
    assert!(matches!(empty, Err(CartError::EmptyCart)));

    cmd.add_line(&id, aoc_monitor()).await?;
    let no_customer = cmd.checkout(&id, checkout_in_3x(None)).await;
    assert!(matches!(no_customer, Err(CartError::CustomerRequired)));

    let duplicate = cmd.add_line(&id, aoc_monitor()).await;
    assert!(matches!(duplicate, Err(CartError::LineAlreadyInCart(_))));

    let usd = cmd
        .add_line(
            &id,
            AddLine {
                unit_price: Money::new(100, "USD"),
                ..headset()
            },
        )
        .await;
    assert!(matches!(usd, Err(CartError::Money(_))));

    let too_many = cmd
        .checkout(
            &id,
            Checkout {
                payment_mode: PaymentMode::Installments { count: 12 },
                ..checkout_in_3x(Some("customer-1"))
            },
        )
        .await;
    assert!(matches!(too_many, Err(CartError::InvalidPaymentMode)));

    Ok(())
}
