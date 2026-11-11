//! What AliExpress pushes to the app's one callback (its "message push"
//! service): the dropshipping topic `DROPSHIPPER_ORDER_STATUS_UPDATE` tells
//! of a buyer order whose status changed. The body is signed with the app's
//! key and secret (HMAC-SHA256 over the key followed by the raw body, in
//! lower-case hex, in the `Authorization` header); a push is delivered at
//! least once, within 500 ms of a 200, and retried twelve times otherwise.
//! Only the order's id and its new status come with it: the order is read
//! again to learn the rest.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// The `message_type` of a dropshipping order's status change, as
/// AliExpress documents it (and warns it may change).
pub const ORDER_STATUS_MESSAGE_TYPE: i64 = 53;

/// What a push says of an order AliExpress closed without shipping it.
pub const ORDER_CLOSED: &str = "OrderClosed";

/// What a push says of an order whose goods the buyer confirmed receiving:
/// its parcels are delivered, and followed no further. The name is the
/// webhook documentation's; the real-app test (#253) is to show it comes.
pub const ORDER_CONFIRMED: &str = "OrderConfirmed";

/// Whether `authorization` signs `body` for the app: HMAC-SHA256 of the app
/// key followed by the raw body, keyed by the app secret, in hex (either
/// case), compared in constant time.
pub fn push_signed(app_key: &str, app_secret: &str, body: &[u8], authorization: &str) -> bool {
    let Ok(given) = hex_decode(authorization.trim()) else {
        return false;
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(app_key.as_bytes());
    mac.update(body);
    mac.verify_slice(&given).is_ok()
}

/// The signature AliExpress sends for `body` (what a stand-in computes).
pub fn push_signature(app_key: &str, app_secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(app_key.as_bytes());
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_decode(text: &str) -> Result<Vec<u8>, ()> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(());
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).map_err(|_| ()))
        .collect()
}

/// One push, read leniently: whatever else it carries is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Push {
    /// A buyer order's status changed.
    OrderStatus {
        ae_order_id: String,
        /// `OrderCreated`, `PaymentAuthorized`, `OrderShipped`,
        /// `OrderConfirmed`, `OrderClosed`, `paymentFailedEvent`, or
        /// whatever AliExpress adds.
        status: String,
    },
    /// Another topic.
    Other { message_type: i64 },
}

#[derive(serde::Deserialize)]
struct PushBody {
    message_type: i64,
    #[serde(default)]
    data: PushData,
}

#[derive(serde::Deserialize, Default)]
struct PushData {
    #[serde(rename = "orderId", default)]
    order_id: Option<serde_json::Value>,
    #[serde(rename = "orderStatus", default)]
    order_status: Option<String>,
}

/// Reads a push; `None` when the body is not one.
pub fn parse_push(body: &[u8]) -> Option<Push> {
    let body: PushBody = serde_json::from_slice(body).ok()?;
    if body.message_type != ORDER_STATUS_MESSAGE_TYPE {
        return Some(Push::Other {
            message_type: body.message_type,
        });
    }
    // The id is a number in the documented example; a string is taken too.
    let ae_order_id = match body.data.order_id? {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text,
        _ => return None,
    };
    Some(Push::OrderStatus {
        ae_order_id,
        status: body.data.order_status.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = br#"{"data":{"buyerId":1861703697,"orderId":1105933950463697,"orderStatus":"OrderShipped"},"message_type":53,"seller_id":"1234","site":"ae_global","timestamp":1719389200}"#;

    #[test]
    fn signs_key_then_body_in_lower_hex() {
        let signature = push_signature("123456", "3412gyo124goi3124", BODY);
        assert_eq!(signature.len(), 64);
        assert!(
            signature
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert!(push_signed("123456", "3412gyo124goi3124", BODY, &signature));
        assert!(push_signed(
            "123456",
            "3412gyo124goi3124",
            BODY,
            &signature.to_uppercase()
        ));
        assert!(push_signed(
            "123456",
            "3412gyo124goi3124",
            BODY,
            &format!(" {signature}\n")
        ));
    }

    #[test]
    fn refuses_another_key_secret_body_or_a_malformed_signature() {
        let signature = push_signature("123456", "3412gyo124goi3124", BODY);
        assert!(!push_signed(
            "654321",
            "3412gyo124goi3124",
            BODY,
            &signature
        ));
        assert!(!push_signed("123456", "other", BODY, &signature));
        assert!(!push_signed(
            "123456",
            "3412gyo124goi3124",
            b"{}",
            &signature
        ));
        assert!(!push_signed("123456", "3412gyo124goi3124", BODY, "not hex"));
        assert!(!push_signed("123456", "3412gyo124goi3124", BODY, ""));
        assert!(!push_signed(
            "123456",
            "3412gyo124goi3124",
            BODY,
            &signature[..62]
        ));
    }

    #[test]
    fn reads_an_order_status_push() {
        assert_eq!(
            parse_push(BODY),
            Some(Push::OrderStatus {
                ae_order_id: "1105933950463697".into(),
                status: "OrderShipped".into(),
            })
        );
        assert_eq!(
            parse_push(br#"{"message_type":53,"data":{"orderId":"8100000000000001"}}"#),
            Some(Push::OrderStatus {
                ae_order_id: "8100000000000001".into(),
                status: String::new(),
            })
        );
    }

    #[test]
    fn tells_other_topics_and_refuses_what_is_not_a_push() {
        assert_eq!(
            parse_push(br#"{"message_type":1,"data":{"trade_order_id":"3016"}}"#),
            Some(Push::Other { message_type: 1 })
        );
        assert_eq!(parse_push(br#"{"message_type":53,"data":{}}"#), None);
        assert_eq!(parse_push(b"not json"), None);
        assert_eq!(parse_push(b"[]"), None);
    }
}
