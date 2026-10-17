//! The headless components: each renders the one element a behaviour needs
//! (a form posting to the library's action with its hidden fields, an input
//! the checkout's script reads, the drawer's root with its `data-*`
//! contract) and nothing else. Every class and attribute comes from the
//! app through `attrs`, and the visible markup through the child nodes:
//!
//! ```ignore
//! add_to_cart_form(product: &product, quantity: Some(1), attrs: attributes! { class="buy" },
//!     <button>"Add to cart"</button>
//! )
//! ```

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_view::{Attributes, Child, View};
use topcoat_view_macro::{class, component, view};

use crate::cart::{CartLineView, OfferView};
use crate::pages::{CartDrawerModel, CheckoutModel, ShippingChoice, SignInModel, offer_form_id};
use crate::query::Product;
use crate::shoppers::SavedAddress;
use crate::storefront::actions;
use crate::urls::{Back, current_path};

// =============================================================================
// The cart
// =============================================================================

/// The form that adds `product` to the cart: `slug`, and `quantity` when
/// given (else the child supplies a `quantity` input), the variant when
/// known (else the child supplies one `option:<id>` menu per option, see
/// [`option_field`]), `offer` and `back` for an offer's card, and `from`
/// (this page unless given), where the shopper comes back with the drawer
/// open. Carries `data-add-to-cart`, which the cart script sends without
/// leaving the page.
#[component]
pub async fn add_to_cart_form(
    cx: &Cx,
    product: &Product,
    #[default] variant_id: Option<&str>,
    #[default] quantity: Option<u32>,
    #[default] offer: Option<&str>,
    #[default] back: Option<Back>,
    #[default] from: Option<&str>,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).cart_add();
    let from = from.map(str::to_owned).unwrap_or_else(|| current_path(cx));
    let back = back.map(Back::as_str).filter(|back| !back.is_empty());
    Ok(view! {
        <form method="post" action=(action) data-add-to-cart="" class=(class) (attrs)>
            <input type="hidden" name="slug" value=(&product.slug)>
            if let Some(id) = variant_id {
                <input type="hidden" name="variant_id" value=(id)>
            }
            if let Some(quantity) = quantity {
                <input type="hidden" name="quantity" value=(quantity)>
            }
            if let Some(offer) = offer {
                <input type="hidden" name="offer" value=(offer)>
            }
            if let Some(back) = back {
                <input type="hidden" name="back" value=(back)>
            }
            <input type="hidden" name="from" value=(from)>
            (child)
        </form>
    })
}

/// The name of the menu that picks a value of `option` in an
/// [`add_to_cart_form`]: `option:<option id>`.
pub fn option_field(option: &crate::query::ProductOption) -> String {
    format!("{}{}", crate::OPTION_FIELD_PREFIX, option.id)
}

/// An offer's own form, by id ([`offer_form_id`]): it sits outside the
/// checkout's form (forms do not nest) and a button anywhere submits it
/// with `form=`. Adds one unit at the offer and comes back to the checkout.
#[component]
pub async fn offer_form(
    cx: &Cx,
    offer: &OfferView,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).cart_add();
    Ok(view! {
        <form id=(offer_form_id(offer)) method="post" action=(action) class=(class) (attrs)>
            <input type="hidden" name="slug" value=(offer.slug.as_deref().unwrap_or_default())>
            <input type="hidden" name="variant_id" value=(offer.variant_id.as_deref().unwrap_or_default())>
            <input type="hidden" name="quantity" value="1">
            <input type="hidden" name="offer" value=(&offer.offer_id)>
            <input type="hidden" name="back" value=(Back::Checkout.as_str())>
        </form>
    })
}

/// The form that sets a line's quantity: to `quantity` when given (a −/+
/// button), else to what the child's `quantity` input says. Zero takes the
/// line out. `back` says where the shopper comes back to, `from` the page
/// the drawer sat on (this page unless given).
#[component]
pub async fn cart_quantity_form(
    cx: &Cx,
    line: &CartLineView,
    #[default] quantity: Option<u32>,
    #[default] back: Back,
    #[default] from: Option<&str>,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).cart_quantity();
    let from = from.map(str::to_owned).unwrap_or_else(|| current_path(cx));
    Ok(view! {
        <form method="post" action=(action) data-cart-quantity="" class=(class) (attrs)>
            <input type="hidden" name="product_id" value=(&line.product_id)>
            <input type="hidden" name="variant_id" value=(line.variant_id.as_deref().unwrap_or_default())>
            if let Some(quantity) = quantity {
                <input type="hidden" name="quantity" value=(quantity)>
            }
            <input type="hidden" name="back" value=(back.as_str())>
            <input type="hidden" name="from" value=(from)>
            (child)
        </form>
    })
}

/// The form that takes a line out of the cart.
#[component]
pub async fn cart_remove_form(
    cx: &Cx,
    line: &CartLineView,
    #[default] back: Back,
    #[default] from: Option<&str>,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).cart_remove();
    let from = from.map(str::to_owned).unwrap_or_else(|| current_path(cx));
    Ok(view! {
        <form method="post" action=(action) data-cart-remove="" class=(class) (attrs)>
            <input type="hidden" name="product_id" value=(&line.product_id)>
            <input type="hidden" name="variant_id" value=(line.variant_id.as_deref().unwrap_or_default())>
            <input type="hidden" name="back" value=(back.as_str())>
            <input type="hidden" name="from" value=(from)>
            (child)
        </form>
    })
}

/// The form that puts a discount code on the cart (the child's `code`
/// input) or takes it off (a button named `remove`).
#[component]
pub async fn discount_form(
    cx: &Cx,
    #[default] back: Back,
    #[default] from: Option<&str>,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).cart_discount();
    let from = from.map(str::to_owned).unwrap_or_else(|| current_path(cx));
    Ok(view! {
        <form method="post" action=(action) data-discount-form="" class=(class) (attrs)>
            <input type="hidden" name="back" value=(back.as_str())>
            <input type="hidden" name="from" value=(from)>
            (child)
        </form>
    })
}

/// The drawer's root: `id="cart"` (the `#cart` fragment opens it without
/// the script) and everything the cart script reads from `data-*`. The
/// child is the drawer's content. Closing links and buttons inside carry
/// `data-drawer-close`; the header's cart link carries `data-cart-link`
/// and its count `data-cart-count`.
#[component]
pub async fn cart_drawer_root(
    drawer: &CartDrawerModel,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <div id="cart" data-cart-drawer="" data-count=(drawer.count)
            data-publishable-key=(&drawer.publishable_key) data-amount=(drawer.total_cents)
            data-currency=(drawer.currency.to_lowercase()) data-countries=(&drawer.countries)
            data-shipping=(&drawer.rates) data-start-path=(&drawer.start_url)
            data-return-path=(&drawer.return_url) data-money=(&drawer.money)
            data-color-primary=(&drawer.accent_color)
            data-msg-standard-shipping=(&drawer.standard_shipping)
            data-msg-error=(&drawer.something_went_wrong)
            class=(class) (attrs)>
            (child)
        </div>
    })
}

/// Where the drawer's wallet buttons mount, hidden until a wallet is
/// available; the child is what shows with them (a [`drawer_pay_error`]).
#[component]
pub async fn drawer_express(
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <div data-drawer-express="" hidden="" class=(class) (attrs)>
            <div id="drawer-express-checkout"></div>
            (child)
        </div>
    })
}

/// Where the drawer's script says a wallet payment failed.
#[component]
pub async fn drawer_pay_error(#[default] mut attrs: Attributes) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <p role="alert" data-drawer-pay-error="" hidden="" class=(class) (attrs)></p>
    })
}

// =============================================================================
// The checkout
// =============================================================================

/// The checkout's form, with everything its script reads from `data-*` and
/// the total the shopper saw as `expected_total_cents`. The child holds the
/// fields, named as [`CheckoutForm`](crate::CheckoutForm) reads them:
/// `email`, `name`, `phone`, `line1`, `line2`, `postal_code`, `city`, a
/// [`country_select`], `note`, `vat_number`, the [`shipping_method_input`]s,
/// a [`billing_toggle`] and [`billing_fieldset`], [`save_address_checkbox`]es,
/// the [`payment_element`] and a [`place_button`].
#[component]
pub async fn checkout_form(
    page: &CheckoutModel,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <form method="post" action=(&page.place_url) data-checkout-form=""
            data-start-path=(&page.start_url) data-return-path=(&page.return_url)
            data-publishable-key=(&page.publishable_key) data-amount=(page.total_cents)
            data-goods=(page.goods_cents) data-shipping=(&page.rates)
            data-currency=(page.currency.to_lowercase()) data-money=(&page.money)
            data-seller-country=(&page.seller_country) data-color-primary=(&page.accent_color)
            data-msg-standard-shipping=(&page.standard_shipping)
            data-msg-error=(&page.something_went_wrong)
            class=(class) (attrs)>
            <input type="hidden" name="expected_total_cents" value=(page.total_cents)>
            (child)
        </form>
    })
}

/// The country menu (`country`, or `billing_country` with `billing`), each
/// option carrying what the VAT line says for it.
#[component]
pub async fn country_select(
    page: &CheckoutModel,
    #[default] billing: bool,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let name = if billing {
        "billing_country"
    } else {
        "country"
    };
    let autocomplete = if billing {
        "billing country"
    } else {
        "country"
    };
    let current = if billing {
        page.billing_country.clone()
    } else {
        page.country.clone()
    };
    Ok(view! {
        <select name=(name) required="" autocomplete=(autocomplete) class=(class) (attrs)>
            for country in &page.countries {
                <option value=(&country.code) selected=(country.code == current)
                    data-vat=(if billing { None } else { Some(country.vat_json.as_str()) })>
                    (&country.name)
                </option>
            }
        </select>
    })
}

/// One shipping method's radio button, with what the script needs to price
/// the order and show only the methods that ship to the country picked.
#[component]
pub async fn shipping_method_input(
    choice: &ShippingChoice,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <input type="radio" name="shipping_method" value=(&choice.option.id)
            data-amount=(choice.option.amount_cents) data-price=(&choice.price)
            data-countries=(choice.option.countries.join(" "))
            checked=(choice.picked) disabled=(!choice.ships_here)
            class=(class) (attrs)>
    })
}

/// The box that asks for a different billing address.
#[component]
pub async fn billing_toggle(
    page: &CheckoutModel,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <input type="checkbox" name="billing_different" value="1"
            checked=(page.bills_elsewhere) class=(class) (attrs)>
    })
}

/// The billing address fields, which count only while the box is ticked:
/// the child names them `billing_name`, `billing_line1`, `billing_line2`,
/// `billing_postal_code`, `billing_city` and a `country_select(billing: true)`.
#[component]
pub async fn billing_fieldset(
    page: &CheckoutModel,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <fieldset data-billing="" hidden=(!page.bills_elsewhere) disabled=(!page.bills_elsewhere)
            class=(class) (attrs)>
            (child)
        </fieldset>
    })
}

/// The box that keeps the address (or the billing one) on the account.
#[component]
pub async fn save_address_checkbox(
    #[default] billing: bool,
    #[default] checked: bool,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let name = if billing {
        "save_billing_address"
    } else {
        "save_address"
    };
    Ok(view! {
        <input type="checkbox" name=(name) value="1" checked=(checked) class=(class) (attrs)>
    })
}

/// The express payment section, hidden until a wallet is available; the
/// child holds an [`express_mount`].
#[component]
pub async fn express_section(
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <section data-express="" hidden="" class=(class) (attrs)>(child)</section>
    })
}

/// Where the wallet buttons mount.
#[component]
pub async fn express_mount(#[default] mut attrs: Attributes) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! { <div id="express-checkout" class=(class) (attrs)></div> })
}

/// Where the provider's card form mounts.
#[component]
pub async fn payment_element(#[default] mut attrs: Attributes) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! { <div id="payment-element" class=(class) (attrs)></div> })
}

/// Where the checkout's script says a payment was refused.
#[component]
pub async fn payment_error(#[default] mut attrs: Attributes) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <p role="alert" data-payment-error="" hidden="" class=(class) (attrs)></p>
    })
}

/// The button that pays; a `<span data-place-total>` in the child shows the
/// total the script keeps current.
#[component]
pub async fn place_button(
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <button data-place="" class=(class) (attrs)>(child)</button>
    })
}

// =============================================================================
// The account
// =============================================================================

/// The form that asks for a sign-in code: the child's `email` input.
#[component]
pub async fn sign_in_form(
    cx: &Cx,
    page: &SignInModel,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).sign_in_request();
    Ok(view! {
        <form method="post" action=(action) data-sign-in-form="" class=(class) (attrs)>
            <input type="hidden" name="next" value=(&page.next)>
            (child)
        </form>
    })
}

/// The form that enters the code: the child's `code` input.
#[component]
pub async fn sign_in_code_form(
    cx: &Cx,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).sign_in_verify();
    Ok(view! {
        <form method="post" action=(action) data-sign-in-code-form="" class=(class) (attrs)>
            (child)
        </form>
    })
}

/// The form that asks for a new code.
#[component]
pub async fn resend_code_form(
    cx: &Cx,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).sign_in_verify();
    Ok(view! {
        <form method="post" action=(action) data-resend-code-form="" class=(class) (attrs)>
            <input type="hidden" name="resend" value="1">
            (child)
        </form>
    })
}

/// The form that signs out of this browser.
#[component]
pub async fn sign_out_form(
    cx: &Cx,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).sign_out();
    Ok(view! {
        <form method="post" action=(action) data-sign-out-form="" class=(class) (attrs)>(child)</form>
    })
}

/// The form that signs out of every other browser.
#[component]
pub async fn sign_out_everywhere_form(
    cx: &Cx,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).sign_out_everywhere();
    Ok(view! {
        <form method="post" action=(action) data-sign-out-everywhere-form="" class=(class) (attrs)>(child)</form>
    })
}

/// The form that deletes the account.
#[component]
pub async fn delete_account_form(
    cx: &Cx,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).account_delete();
    Ok(view! {
        <form method="post" action=(action) data-delete-account-form="" class=(class) (attrs)>(child)</form>
    })
}

/// The form that forgets a saved address.
#[component]
pub async fn remove_address_form(
    cx: &Cx,
    saved: &SavedAddress,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).address_remove();
    Ok(view! {
        <form method="post" action=(action) data-remove-address-form="" class=(class) (attrs)>
            <input type="hidden" name="address_id" value=(&saved.id)>
            (child)
        </form>
    })
}

// =============================================================================
// Reviews
// =============================================================================

/// The form that hands in a review of `product_id` from `order_id`: the
/// child names `rating`, `title`, `body` and `photo` (a file input,
/// `multiple`).
#[component]
pub async fn review_form(
    cx: &Cx,
    order_id: &str,
    product_id: &str,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).review_submit();
    Ok(view! {
        <form method="post" enctype="multipart/form-data" action=(action) data-review-form=""
            class=(class) (attrs)>
            <input type="hidden" name="order_id" value=(order_id)>
            <input type="hidden" name="product_id" value=(product_id)>
            (child)
        </form>
    })
}

/// The form that changes a review while it waits: like [`review_form`],
/// plus one `keep_photo` box per photo to keep.
#[component]
pub async fn revise_review_form(
    cx: &Cx,
    order_id: &str,
    product_id: &str,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let action = actions(cx).review_revise();
    Ok(view! {
        <form method="post" enctype="multipart/form-data" action=(action) data-revise-review-form=""
            class=(class) (attrs)>
            <input type="hidden" name="order_id" value=(order_id)>
            <input type="hidden" name="product_id" value=(product_id)>
            (child)
        </form>
    })
}
