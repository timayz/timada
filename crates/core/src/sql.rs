//! Bits of SQL every read model writes the same way.

/// A `LIKE` pattern matching `q` anywhere, its wildcards escaped: to be
/// used with `ESCAPE '\\'`.
pub fn like_contains(q: &str) -> String {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns_escape_wildcards() {
        assert_eq!(like_contains("mug"), "%mug%");
        assert_eq!(like_contains("50%_off\\"), "%50\\%\\_off\\\\%");
    }
}
