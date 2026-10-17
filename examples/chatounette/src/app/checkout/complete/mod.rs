//! `/checkout/complete`: where Stripe sends the browser back. The order
//! exists once Stripe told the dashboard the payment went through, which may
//! take a moment longer than the browser's way back: until then the page
//! says so and looks again.

use timada_storefront::pages::checkout_complete;
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{Unescaped, View, view},
};

use crate::app::document;

/// Looks again in two seconds.
const RELOAD_SCRIPT: &str = "setTimeout(() => location.reload(), 2000);";

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let waiting = checkout_complete(cx).await?;

    Ok(view! {
        document(
            title: "Confirmation du paiement",
            <h1>"Confirmation de votre paiement…"</h1>
            <p data-payment-confirming="">"Cela prend quelques secondes. La page se rafraîchit toute seule."</p>
            <p><a href=(&waiting.retry_url)>"Vérifier à nouveau"</a></p>
            <script>(Unescaped::new_unchecked(RELOAD_SCRIPT))</script>
        )
    })
}
