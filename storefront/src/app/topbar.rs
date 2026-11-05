//! The bar above the masthead: the announcements the merchant writes in the
//! back office ([`timada_storefront::announcements`]), those whose window is
//! open now, in the order set there. No announcement, no bar.
//!
//! Without JS the first one shows. With several, [`TOPBAR_SCRIPT`] shows
//! them in turn, every five seconds, and holds while the pointer or the
//! focus is on the bar, or once the shopper presses Pause (WCAG 2.2.2). The
//! bar is not a live region: a screen reader is not interrupted each turn.

use timada_storefront::LiveAnnouncement;
use topcoat::{
    Result,
    view::{Unescaped, View, view},
};

/// Shows the next announcement every five seconds.
pub const TOPBAR_SCRIPT: &str = r#"
(() => {
  const bar = document.querySelector("[data-announcements]");
  if (!bar) return;
  const items = [...bar.querySelectorAll("[data-announcement]")];
  const pause = bar.querySelector("[data-announcements-pause]");
  if (items.length < 2 || !pause) return;
  let current = 0, paused = false, held = false;
  pause.hidden = false;
  pause.addEventListener("click", () => {
    paused = !paused;
    pause.setAttribute("aria-pressed", String(paused));
    pause.textContent = paused ? "Reprendre" : "Pause";
  });
  const hold = (on) => () => { held = on || bar.matches(":hover") || bar.contains(document.activeElement); };
  bar.addEventListener("mouseenter", hold(true));
  bar.addEventListener("mouseleave", hold(false));
  bar.addEventListener("focusin", hold(true));
  bar.addEventListener("focusout", () => setTimeout(hold(false)));
  setInterval(() => {
    if (paused || held || document.hidden) return;
    items[current].hidden = true;
    current = (current + 1) % items.length;
    items[current].hidden = false;
  }, 5000);
})();
"#;

/// The bar, or nothing when no announcement is live.
#[topcoat::view::component]
pub async fn announcement_bar(announcements: &[LiveAnnouncement]) -> Result<impl View> {
    Ok(view! {
        if !announcements.is_empty() {
            <div class="topbar" role="region" aria-label="Annonces" data-announcements="">
                <div class="wrap">
                    for (index, announcement) in announcements.iter().enumerate() {
                        <p data-announcement=(&announcement.id) hidden=(index > 0)>
                            (Unescaped::new_unchecked(announcement.html.clone()))
                        </p>
                    }
                    if announcements.len() > 1 {
                        <button type="button" class="topbar-pause" aria-pressed="false" data-announcements-pause="" hidden="">"Pause"</button>
                    }
                </div>
            </div>
            if announcements.len() > 1 {
                <script>(Unescaped::new_unchecked(TOPBAR_SCRIPT))</script>
            }
        }
    })
}
