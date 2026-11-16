//! The pop-up that invites a visitor to the newsletter, when the merchant
//! set one in the back office: a `<dialog>` every page carries, closed,
//! which a small script opens after the delay the merchant chose, or when
//! the pointer leaves the page upwards. It opens once for a visitor (the
//! script notes it in the `newsletter` cookie) and never for one who gave
//! their address; without JavaScript it never opens. Its form is the
//! footer's: the address is mailed a link to confirm, as from anywhere
//! else.

use timada_storefront::{NewsletterPopup, newsletter_popup, takes_newsletter_signups};
use topcoat::{
    Result,
    context::Cx,
    router::{href, request::original_uri},
    view::{Unescaped, View, view},
};

use crate::app::{newsletter, sitemap};
use crate::shopper::{NEWSLETTER_COOKIE, invited};

/// The pop-up this page carries for this visitor, if any: the merchant
/// shows one, the shop takes sign-ups, the visitor has not seen it and has
/// not subscribed, and the page is one a shopper reads, not the cart, the
/// checkout or their account.
pub async fn popup_for(cx: &Cx) -> Result<Option<NewsletterPopup>> {
    if invited(cx) || !takes_newsletter_signups(cx) || sitemap::is_private(original_uri(cx).path())
    {
        return Ok(None);
    }
    newsletter_popup(cx).await
}

/// Opens the dialog once: after its delay, or as the pointer leaves the
/// page upwards. Not while the cookie banner still asks its question, nor
/// over the cart drawer: the next page tries again.
const POPUP_SCRIPT: &str = r#"(function () {
  var dialog = document.querySelector("[data-newsletter-popup]");
  if (!dialog || !dialog.showModal) return;
  var cookie = dialog.getAttribute("data-cookie");
  var done = false;
  function open() {
    if (done || document.querySelector("[data-consent], [data-cart-drawer].open")) return;
    done = true;
    document.cookie = cookie + "=seen; path=/; max-age=31536000; samesite=lax";
    dialog.showModal();
  }
  setTimeout(open, Number(dialog.getAttribute("data-delay")) * 1000);
  if (dialog.hasAttribute("data-on-leave")) {
    document.documentElement.addEventListener("mouseleave", function (event) {
      if (event.clientY <= 0) open();
    });
  }
})();"#;

/// The layout of the dialog.
pub const POPUP_CSS: &str = "dialog.popup { width: min(28rem, 100% - 2rem); padding: 1.5rem; border: 1px solid; }
dialog.popup h2 { font-size: 1.3rem; margin: 0 2rem .5rem 0; }
dialog.popup p { margin: 0 0 1rem; }
dialog.popup .popup-close { position: absolute; top: .75rem; right: .75rem; margin: 0; }
dialog.popup .popup-close button { border: 1px solid; background: none; font: inherit; padding: .2rem .55rem; cursor: pointer; }
dialog.popup .newsletter-row input { flex: 1 1 8rem; min-width: 0; padding: .6rem .75rem; border: 1px solid; }";

#[topcoat::view::component]
pub async fn newsletter_dialog(cx: &Cx, popup: &NewsletterPopup) -> Result<impl View> {
    Ok(view! {
        <dialog class="popup" aria-labelledby="popup-title" data-newsletter-popup=""
            data-delay=(popup.delay_secs) data-on-leave=(popup.on_leave.then_some(""))
            data-cookie=(NEWSLETTER_COOKIE)>
            <form class="popup-close" method="dialog">
                <button aria-label="Fermer">"×"</button>
            </form>
            <h2 id="popup-title">(&popup.title)</h2>
            if !popup.text.is_empty() {
                <p>(&popup.text)</p>
            }
            <form class="newsletter" method="post" action=(href!(newsletter::subscribe))>
                <label for="popup-email">"Votre e-mail"</label>
                <div class="newsletter-row">
                    <input id="popup-email" name="email" type="email" required="" autocomplete="email" placeholder="vous@exemple.fr">
                    <button class="btn">"S'inscrire"</button>
                </div>
            </form>
        </dialog>
        <script>(Unescaped::new_unchecked(POPUP_SCRIPT))</script>
    })
}
