//! Small pure text helpers.

/// Truncate a string to a maximum length, adding `...` if needed.
///
/// - Trims surrounding whitespace before truncating.
/// - Uses `char` count (not bytes) to avoid splitting Unicode scalar values.
/// - Enforces a minimum `max` of 3 so the ellipsis fits.
#[must_use]
pub fn truncate_with_ellipsis(raw: &str, max: usize) -> String {
    let max = max.max(3);
    let trimmed = raw.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max - 3).collect();
        format!("{head}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate_with_ellipsis("hello world", 8), "hello...");
    }

    #[test]
    fn truncate_trims_whitespace() {
        assert_eq!(truncate_with_ellipsis("  hello  ", 10), "hello");
    }

    #[test]
    fn truncate_min_length_is_three() {
        // Even with max=1, we should get at least "..."
        assert_eq!(truncate_with_ellipsis("hello", 1), "...");
    }
}
