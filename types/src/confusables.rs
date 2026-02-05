//! Homoglyph and confusable character detection.
//!
//! This module detects mixed-script content that could indicate homoglyph attacks,
//! where visually-similar characters from different Unicode scripts are used to
//! create deceptive text (e.g., Cyrillic 'а' looks like Latin 'a').
//!
//! # IFA Conformance
//!
//! - **IFA-8 (Mechanism vs Policy)**: [`detect_mixed_script`] is a **mechanism** that
//!   reports the fact "this string contains mixed scripts". The caller (UI) makes
//!   the **policy** decision about how to display the warning.
//! - **IFA-11 (Boundary/Core)**: Analysis happens at the boundary (when preparing
//!   approval requests). [`HomoglyphWarning`] is a proof object that analysis was
//!   performed. The UI receives proof objects and renders them without re-analyzing.

use unicode_script::{Script, UnicodeScript};

/// Proof that homoglyph analysis was performed and detected suspicious content.
///
/// The existence of this type proves that the analysis detected a potential
/// homoglyph attack vector (mixed scripts in a high-risk field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomoglyphWarning {
    /// The field name where mixed scripts were detected (e.g., "url", "command").
    pub field_name: String,
    /// A truncated snippet of the suspicious content for display.
    pub snippet: String,
    /// The scripts detected in the content.
    pub scripts: Vec<Script>,
}

impl HomoglyphWarning {
    /// Format scripts for human-readable display.
    ///
    /// Returns a comma-separated list of script names.
    #[must_use]
    pub fn scripts_display(&self) -> String {
        self.scripts
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Analyze a string for mixed-script content (MECHANISM per IFA-8).
///
/// Returns `Some(HomoglyphWarning)` if suspicious mixed-script content is detected,
/// `None` otherwise. The warning is a proof object that detection occurred.
///
/// # Detection Logic
///
/// Only flags Latin mixed with Cyrillic or Greek (highest attack surface for
/// English-language tools). Pure non-Latin scripts (legitimate non-English content)
/// are not flagged.
///
/// # Fast Path
///
/// ASCII-only strings return `None` immediately without character iteration.
///
/// # Examples
///
/// ```
/// use forge_types::detect_mixed_script;
///
/// // Cyrillic 'а' (U+0430) looks like Latin 'a'
/// let warning = detect_mixed_script("pаypal.com", "url");
/// assert!(warning.is_some());
///
/// // Pure Latin is fine
/// assert!(detect_mixed_script("paypal.com", "url").is_none());
///
/// // Pure Cyrillic is fine (legitimate Russian content)
/// assert!(detect_mixed_script("привет", "text").is_none());
/// ```
#[must_use]
pub fn detect_mixed_script(input: &str, field_name: &str) -> Option<HomoglyphWarning> {
    // Fast path: ASCII-only strings cannot have mixed scripts
    if input.is_ascii() {
        return None;
    }

    let mut has_latin = false;
    let mut has_cyrillic = false;
    let mut has_greek = false;

    for c in input.chars() {
        match c.script() {
            Script::Latin => has_latin = true,
            Script::Cyrillic => has_cyrillic = true,
            Script::Greek => has_greek = true,
            _ => {}
        }
    }

    // Only warn on Latin mixed with Cyrillic or Greek (highest attack surface)
    // Pure Cyrillic or pure Greek are legitimate (non-English content)
    let suspicious = has_latin && (has_cyrillic || has_greek);
    if !suspicious {
        return None;
    }

    let mut scripts = vec![Script::Latin];
    if has_cyrillic {
        scripts.push(Script::Cyrillic);
    }
    if has_greek {
        scripts.push(Script::Greek);
    }

    Some(HomoglyphWarning {
        field_name: field_name.to_string(),
        snippet: truncate_for_display(input, 40),
        scripts,
    })
}

/// Truncate a string for display, adding "..." if truncated.
fn truncate_for_display(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_latin_cyrillic_mix() {
        // Cyrillic 'а' (U+0430) looks like Latin 'a'
        let warning = detect_mixed_script("pаypal.com", "url");
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert!(w.scripts.contains(&Script::Cyrillic));
        assert!(w.scripts.contains(&Script::Latin));
        assert_eq!(w.field_name, "url");
    }

    #[test]
    fn detects_latin_greek_mix() {
        // Greek 'ο' (U+03BF) looks like Latin 'o'
        let warning = detect_mixed_script("gοogle.com", "url");
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert!(w.scripts.contains(&Script::Greek));
        assert!(w.scripts.contains(&Script::Latin));
    }

    #[test]
    fn ignores_pure_latin() {
        let warning = detect_mixed_script("google.com", "url");
        assert!(warning.is_none());
    }

    #[test]
    fn ignores_pure_cyrillic() {
        // Legitimate Russian content
        let warning = detect_mixed_script("привет", "text");
        assert!(warning.is_none());
    }

    #[test]
    fn ignores_pure_greek() {
        // Legitimate Greek content
        let warning = detect_mixed_script("γεια", "text");
        assert!(warning.is_none());
    }

    #[test]
    fn ignores_ascii_only_fast_path() {
        let warning = detect_mixed_script("https://example.com/path?q=test", "url");
        assert!(warning.is_none());
    }

    #[test]
    fn truncates_long_snippets() {
        // Create a long string with mixed scripts
        let long_input = format!("{}а", "a".repeat(100)); // 100 Latin 'a' + Cyrillic 'а'
        let warning = detect_mixed_script(&long_input, "field");
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert!(w.snippet.ends_with("..."));
        assert!(w.snippet.len() <= 43 + 3); // 40 chars + "..."
    }

    #[test]
    fn scripts_display_formats_correctly() {
        let warning = HomoglyphWarning {
            field_name: "test".to_string(),
            snippet: "test".to_string(),
            scripts: vec![Script::Latin, Script::Cyrillic],
        };
        let display = warning.scripts_display();
        assert!(display.contains("Latin"));
        assert!(display.contains("Cyrillic"));
        assert!(display.contains(", "));
    }

    #[test]
    fn handles_empty_string() {
        let warning = detect_mixed_script("", "field");
        assert!(warning.is_none());
    }

    #[test]
    fn handles_unicode_without_latin() {
        // Japanese + Cyrillic should not warn (no Latin)
        let warning = detect_mixed_script("日本語привет", "text");
        assert!(warning.is_none());
    }

    #[test]
    fn detects_single_cyrillic_in_latin() {
        // Single Cyrillic 'е' (U+0435) among Latin
        let warning = detect_mixed_script("tеst", "command");
        assert!(warning.is_some());
    }

    #[test]
    fn preserves_field_name() {
        let warning = detect_mixed_script("tеst", "my_custom_field").unwrap();
        assert_eq!(warning.field_name, "my_custom_field");
    }
}
