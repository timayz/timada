use std::collections::BTreeMap;

use timada_aliexpress::{authorize_url, sign};

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

// The expected signatures were worked out apart from this crate, the way
// moh3a/ae_sdk signs: HMAC-SHA256 over the path (for a path call) then the
// sorted `name` + `value` pairs, in upper-case hex.

#[test]
fn a_path_call_signs_its_path_first() {
    let signed = sign(
        "s3cret",
        Some("/auth/token/create"),
        &params(&[
            ("timestamp", "1700000000000"),
            ("code", "abc"),
            ("app_key", "12345"),
            ("sign_method", "sha256"),
            ("simplify", "true"),
        ]),
    );
    assert_eq!(
        signed,
        "1A5DE3AE56CF94570EDB35D8DF699A36CACC4F65B424AA3B9DA61B10DB8A224B"
    );
}

#[test]
fn a_method_call_signs_its_method_among_the_parameters() {
    let signed = sign(
        "s3cret",
        None,
        &params(&[
            ("method", "aliexpress.ds.product.get"),
            ("product_id", "1005006"),
            ("session", "tok"),
            ("app_key", "12345"),
            ("sign_method", "sha256"),
            ("simplify", "true"),
            ("timestamp", "1700000000000"),
        ]),
    );
    assert_eq!(
        signed,
        "ED2F4E7C048BFD5DD5C56D836A6613ADFFA90DC2155A1B465D08D94DE3EA6BFD"
    );
}

#[test]
fn the_authorize_address_names_the_app_and_the_way_back() {
    let url = authorize_url(
        "https://api-sg.aliexpress.com/oauth/authorize",
        "12345",
        "https://timada.localhost/t/shop/aliexpress/callback",
        "st4te",
    );
    assert!(url.starts_with("https://api-sg.aliexpress.com/oauth/authorize?response_type=code"));
    assert!(url.contains("client_id=12345"));
    assert!(url.contains(
        "redirect_uri=https%3A%2F%2Ftimada.localhost%2Ft%2Fshop%2Faliexpress%2Fcallback"
    ));
    assert!(url.contains("state=st4te"));
}
