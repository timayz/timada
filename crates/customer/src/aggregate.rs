/// Everything that has ever happened to a customer identity.
///
/// Deliberately credential-free: passwords are mutable secrets and live in
/// SQL. Profile edits become additive variants when they arrive.
#[evento::aggregate]
pub enum Customer {
    /// A shopper claimed an email and became a customer.
    CustomerRegistered { email: String, full_name: String },
}
