//! The signed-in shopper's account: their orders, saved addresses, and the
//! way to sign out elsewhere or delete the account. Signed out, both pages
//! send to the sign-in page.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::error::redirect;

use crate::actions::query;
use crate::context::menu;
use crate::query::CategoryNode;
use crate::session::{forget_shopper, shopper_token, signed_in};
use crate::shoppers::{Shopper, ShopperOrder, shopper_orders};
use crate::storefront::{messages, money_format, paths};

use super::{Flash, flash};

#[derive(serde::Deserialize)]
struct AccountQuery {
    error: Option<String>,
    done: Option<String>,
}

/// One of the shopper's orders, as the account's table shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRow {
    pub order: ShopperOrder,
    pub url: String,
    pub status_text: String,
    pub total: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountModel {
    pub nav: Vec<CategoryNode>,
    pub shopper: Shopper,
    pub orders: Vec<OrderRow>,
    pub error: Option<Flash>,
    /// What was just done, in the shop's words.
    pub done: Option<String>,
    pub delete_url: String,
    pub home_url: String,
}

async fn require_signed_in(cx: &Cx, here: String) -> Result<(Shopper, String)> {
    let to_sign_in = paths(cx).sign_in_url(Some(&here), None);
    let Some(shopper) = signed_in(cx).await.cloned() else {
        return Err(redirect(to_sign_in).into());
    };
    let Some(token) = shopper_token(cx) else {
        return Err(redirect(to_sign_in).into());
    };
    Ok((shopper, token))
}

pub async fn account_page(cx: &Cx) -> Result<AccountModel> {
    let paths = paths(cx);
    let here = paths.account_url(None);
    let (shopper, token) = require_signed_in(cx, here.clone()).await?;
    let orders = match shopper_orders(cx, &token).await? {
        Ok(orders) => orders,
        Err(refusal) if refusal.is_signed_out() => {
            forget_shopper(cx);
            return Err(redirect(paths.sign_in_url(Some(&here), None)).into());
        }
        Err(_) => Vec::new(),
    };
    let query: AccountQuery = query(cx)?;
    let messages = messages(cx);
    let money = money_format(cx);
    Ok(AccountModel {
        nav: menu(cx).await?,
        shopper,
        orders: orders
            .into_iter()
            .map(|order| OrderRow {
                url: paths.order_url(&order.id),
                status_text: messages.order_status(order.status()).to_owned(),
                total: money.format(order.total_cents.max(0) as u64, &order.currency),
                order,
            })
            .collect(),
        error: flash(cx, query.error.as_deref()),
        done: query
            .done
            .as_deref()
            .and_then(|code| messages.say_done(code))
            .map(str::to_owned),
        delete_url: paths.account_delete_url(),
        home_url: paths.home_url(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteAccountModel {
    pub nav: Vec<CategoryNode>,
    pub shopper: Shopper,
    /// Back to the account, keeping it.
    pub keep_url: String,
}

pub async fn delete_account_page(cx: &Cx) -> Result<DeleteAccountModel> {
    let paths = paths(cx);
    let (shopper, _) = require_signed_in(cx, paths.account_delete_url()).await?;
    Ok(DeleteAccountModel {
        nav: menu(cx).await?,
        shopper,
        keep_url: paths.account_url(None),
    })
}
