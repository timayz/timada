/// Generate a new ULID string — the id format for every aggregate in Timada.
pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}
