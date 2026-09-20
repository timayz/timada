// The card form of /checkout/pay/{order_id}: Stripe's Payment Element, fed by
// the data attributes of #payment-form. Card data never touches this shop —
// the fields live in Stripe's frames.
(() => {
  const form = document.getElementById("payment-form");
  if (!form) return;
  const button = document.getElementById("payment-submit");
  const problem = document.getElementById("payment-problem");
  const progress = document.getElementById("payment-progress");
  const say = (where, text) => {
    where.textContent = text;
    where.hidden = !text;
  };

  if (!window.Stripe) {
    say(problem, "Le module de paiement n'a pas pu être chargé. Rechargez la page.");
    return;
  }
  const stripe = window.Stripe(form.dataset.publishableKey, { locale: "fr" });
  const clientSecret = form.dataset.clientSecret;

  stripe.retrievePaymentIntent(clientSecret).then(({ paymentIntent, error }) => {
    if (error) {
      say(problem, error.message || "Le paiement est indisponible pour le moment.");
      return;
    }
    // Paid already — back from the bank's page, or another tab: the shop
    // hears it from Stripe in a moment, and this page then moves on.
    if (["succeeded", "processing"].includes(paymentIntent.status)) {
      form.hidden = true;
      say(progress, "Paiement reçu, confirmation de votre commande en cours…");
      setTimeout(() => window.location.reload(), 3000);
      return;
    }

    const elements = stripe.elements({ clientSecret });
    const card = elements.create("payment");
    card.mount("#payment-element");
    card.on("ready", () => {
      button.disabled = false;
    });

    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      button.disabled = true;
      say(problem, "");
      say(progress, "Paiement en cours…");
      // On success Stripe sends the browser to return_url; coming back here
      // means it did not go through.
      const { error: refused } = await stripe.confirmPayment({
        elements,
        confirmParams: { return_url: form.dataset.returnUrl },
      });
      say(progress, "");
      say(problem, refused.message || "Le paiement n'a pas abouti. Réessayez.");
      button.disabled = false;
    });
  });
})();
