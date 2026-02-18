//! Small pure text helpers.

/// Truncate `s` and append `suffix` if it exceeds `threshold` characters.
///
/// - `threshold`: character count at which truncation kicks in.
/// - `take`: how many characters of content to keep when truncating.
fn truncate_core(s: &str, threshold: usize, take: usize, suffix: &str) -> String {
    if s.chars().count() <= threshold {
        return s.to_string();
    }
    let head: String = s.chars().take(take).collect();
    format!("{head}{suffix}")
}

/// Truncate a string to fit within `max_total` characters, appending `suffix` if truncated.
///
/// The suffix counts toward the budget: the returned string is at most `max_total` characters.
#[must_use]
pub fn truncate_to_fit(raw: &str, max_total: usize, suffix: &str) -> String {
    let take = max_total.saturating_sub(suffix.chars().count());
    truncate_core(raw, max_total, take, suffix)
}

/// Truncate a string preserving up to `max_content` characters, then append `suffix`.
///
/// The suffix does NOT count toward the budget: the returned string may be up to
/// `max_content + suffix.chars().count()` characters.
#[must_use]
pub(crate) fn truncate_preview(raw: &str, max_content: usize, suffix: &str) -> String {
    truncate_core(raw, max_content, max_content, suffix)
}

/// Truncate a string to a maximum length, adding `...` if needed.
///
/// - Trims surrounding whitespace before truncating.
/// - Uses `char` count (not bytes) to avoid splitting Unicode scalar values.
/// - Enforces a minimum `max` of 3 so the ellipsis fits.
#[must_use]
pub fn truncate_with_ellipsis(raw: &str, max: usize) -> String {
    let max = max.max(3);
    truncate_to_fit(raw.trim(), max, "...")
}

#[cfg(test)]
mod tests {
    use super::{truncate_preview, truncate_to_fit, truncate_with_ellipsis};

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

    #[test]
    fn to_fit_respects_budget() {
        let result = truncate_to_fit("hello world", 8, "…");
        assert!(result.chars().count() <= 8);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn to_fit_short_unchanged() {
        assert_eq!(truncate_to_fit("hello", 10, "…"), "hello");
    }

    #[test]
    fn preview_suffix_outside_budget() {
        let result = truncate_preview("hello world", 5, "...");
        assert_eq!(result, "hello...");
    }

    #[test]
    fn preview_short_unchanged() {
        assert_eq!(truncate_preview("hello", 10, "..."), "hello");
    }
}
