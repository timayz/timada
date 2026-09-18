/// Derives a deterministic aggregate id from its natural key.
///
/// `kind` discriminates aggregate types that share a natural key (an order's
/// payment, shipment and invoice are all keyed by the order id). Creating an
/// aggregate on a derived id with `evento::append(&id)` at the default original
/// version is an atomic "create unless it already exists".
pub fn derived(parts: &[&str], kind: &str) -> String {
    let mut ids: Vec<String> = parts.iter().map(|p| (*p).to_owned()).collect();
    ids.push(kind.to_owned());
    evento::hash_ids(ids)
}

#[cfg(test)]
mod tests {
    use super::derived;

    #[test]
    fn is_stable_and_kind_scoped() {
        assert_eq!(
            derived(&["order-1"], "payment"),
            derived(&["order-1"], "payment")
        );
        assert_ne!(
            derived(&["order-1"], "payment"),
            derived(&["order-1"], "invoice")
        );
        assert_ne!(derived(&["a", "b"], "x"), derived(&["ab"], "x"));
    }
}
