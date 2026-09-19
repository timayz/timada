use timada_core::Address;

pub use timada_core::format::{date, money};

/// The lines of a postal address, name first.
pub fn address_lines(address: &Address) -> Vec<String> {
    let mut lines = vec![address.full_name(), address.line1.clone()];
    lines.extend(address.line2.clone());
    lines.push(format!("{} {}", address.postal_code, address.city));
    lines.push(address.country_code.clone());
    lines.extend(address.phone.clone());
    lines.extend(address.mobile.clone());
    lines
}

pub fn order_status(status: &str) -> &'static str {
    match status {
        "placed" => "En attente de paiement",
        "paid" => "Payée, en préparation",
        "shipped" => "Expédiée",
        "cancelled" => "Annulée",
        _ => "Inconnu",
    }
}
