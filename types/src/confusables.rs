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

use crate::text::truncate_preview;

/// Mixed-script detection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MixedScriptDetection {
    Clean,
    Suspicious(HomoglyphWarning),
}

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
/// Returns [`MixedScriptDetection::Suspicious`] if suspicious mixed-script
/// content is detected, [`MixedScriptDetection::Clean`] otherwise.
///
/// # Detection Logic
///
/// Only flags Latin mixed with Cyrillic, Greek, Armenian, or Cherokee (highest
/// attack surface for English-language tools). Pure non-Latin scripts (legitimate
/// non-English content) are not flagged.
///
/// # Fast Path
///
/// ASCII-only strings return `MixedScriptDetection::Clean` immediately without character iteration.
///
/// # Examples
///
/// ```
/// use forge_types::{MixedScriptDetection, detect_mixed_script};
///
/// // Cyrillic 'а' (U+0430) looks like Latin 'a'
/// let warning = detect_mixed_script("pаypal.com", "url");
/// assert!(matches!(warning, MixedScriptDetection::Suspicious(_)));
///
/// // Pure Latin is fine
/// assert!(matches!(
///     detect_mixed_script("paypal.com", "url"),
///     MixedScriptDetection::Clean
/// ));
///
/// // Pure Cyrillic is fine (legitimate Russian content)
/// assert!(matches!(
///     detect_mixed_script("привет", "text"),
///     MixedScriptDetection::Clean
/// ));
/// ```
#[must_use]
pub fn detect_mixed_script(input: &str, field_name: &str) -> MixedScriptDetection {
    // Fast path: ASCII-only strings cannot have mixed scripts
    if input.is_ascii() {
        return MixedScriptDetection::Clean;
    }

    let mut has_latin = false;
    let mut has_cyrillic = false;
    let mut has_greek = false;
    let mut has_armenian = false;
    let mut has_cherokee = false;

    for c in input.chars() {
        match c.script() {
            Script::Latin => has_latin = true,
            Script::Cyrillic => has_cyrillic = true,
            Script::Greek => has_greek = true,
            Script::Armenian => has_armenian = true,
            Script::Cherokee => has_cherokee = true,
            _ => {}
        }
    }

    // Only warn on Latin mixed with high-confusability scripts.
    // Pure non-Latin scripts are legitimate (non-English content).
    let suspicious = has_latin && (has_cyrillic || has_greek || has_armenian || has_cherokee);
    if !suspicious {
        return MixedScriptDetection::Clean;
    }

    let mut scripts = vec![Script::Latin];
    if has_cyrillic {
        scripts.push(Script::Cyrillic);
    }
    if has_greek {
        scripts.push(Script::Greek);
    }
    if has_armenian {
        scripts.push(Script::Armenian);
    }
    if has_cherokee {
        scripts.push(Script::Cherokee);
    }

    MixedScriptDetection::Suspicious(HomoglyphWarning {
        field_name: field_name.to_string(),
        // Historical behavior: take 40 chars, then append "..." (suffix outside budget).
        snippet: truncate_preview(input, 40, "..."),
        scripts,
    })
}

#[cfg(test)]
mod tests {
    use super::{HomoglyphWarning, MixedScriptDetection, Script, detect_mixed_script};

    #[test]
    fn detects_latin_cyrillic_mix() {
        // Cyrillic 'а' (U+0430) looks like Latin 'a'
        let warning = detect_mixed_script("pаypal.com", "url");
        let w = match warning {
            MixedScriptDetection::Suspicious(w) => w,
            MixedScriptDetection::Clean => panic!("expected suspicious detection"),
        };
        assert!(w.scripts.contains(&Script::Cyrillic));
        assert!(w.scripts.contains(&Script::Latin));
        assert_eq!(w.field_name, "url");
    }

    #[test]
    fn detects_latin_greek_mix() {
        // Greek 'ο' (U+03BF) looks like Latin 'o'
        let warning = detect_mixed_script("gοogle.com", "url");
        let w = match warning {
            MixedScriptDetection::Suspicious(w) => w,
            MixedScriptDetection::Clean => panic!("expected suspicious detection"),
        };
        assert!(w.scripts.contains(&Script::Greek));
        assert!(w.scripts.contains(&Script::Latin));
    }

    #[test]
    fn ignores_pure_latin() {
        let warning = detect_mixed_script("google.com", "url");
        assert!(matches!(warning, MixedScriptDetection::Clean));
    }

    #[test]
    fn ignores_pure_cyrillic() {
        let warning = detect_mixed_script("привет", "text");
        assert!(matches!(warning, MixedScriptDetection::Clean));
    }

    #[test]
    fn ignores_pure_greek() {
        let warning = detect_mixed_script("γεια", "text");
        assert!(matches!(warning, MixedScriptDetection::Clean));
    }

    #[test]
    fn ignores_ascii_only_fast_path() {
        let warning = detect_mixed_script("https://example.com/path?q=test", "url");
        assert!(matches!(warning, MixedScriptDetection::Clean));
    }

    #[test]
    fn truncates_long_snippets() {
        // Create a long string with mixed scripts
        let long_input = format!("{}а", "a".repeat(100)); // 100 Latin 'a' + Cyrillic 'а'
        let warning = detect_mixed_script(&long_input, "field");
        let w = match warning {
            MixedScriptDetection::Suspicious(w) => w,
            MixedScriptDetection::Clean => panic!("expected suspicious detection"),
        };
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
        assert!(matches!(warning, MixedScriptDetection::Clean));
    }

    #[test]
    fn handles_unicode_without_latin() {
        // Japanese + Cyrillic should not warn (no Latin)
        let warning = detect_mixed_script("日本語привет", "text");
        assert!(matches!(warning, MixedScriptDetection::Clean));
    }

    #[test]
    fn detects_single_cyrillic_in_latin() {
        // Single Cyrillic 'е' (U+0435) among Latin
        let warning = detect_mixed_script("tеst", "command");
        assert!(matches!(warning, MixedScriptDetection::Suspicious(_)));
    }

    #[test]
    fn preserves_field_name() {
        let warning = detect_mixed_script("t\u{0435}st", "my_custom_field");
        let w = match warning {
            MixedScriptDetection::Suspicious(w) => w,
            MixedScriptDetection::Clean => panic!("expected suspicious detection"),
        };
        assert_eq!(w.field_name, "my_custom_field");
    }

    #[test]
    fn detects_latin_armenian_mix() {
        // Armenian 'Ա' (U+0531) mixed with Latin
        let warning = detect_mixed_script("p\u{0561}ypal.com", "url");
        let w = match warning {
            MixedScriptDetection::Suspicious(w) => w,
            MixedScriptDetection::Clean => panic!("expected suspicious detection"),
        };
        assert!(w.scripts.contains(&Script::Armenian));
        assert!(w.scripts.contains(&Script::Latin));
    }

    #[test]
    fn detects_latin_cherokee_mix() {
        // Cherokee 'Ꮪ' (U+13DA) mixed with Latin
        let warning = detect_mixed_script("te\u{13DA}t.com", "url");
        let w = match warning {
            MixedScriptDetection::Suspicious(w) => w,
            MixedScriptDetection::Clean => panic!("expected suspicious detection"),
        };
        assert!(w.scripts.contains(&Script::Cherokee));
        assert!(w.scripts.contains(&Script::Latin));
    }

    #[test]
    fn ignores_pure_armenian() {
        let warning = detect_mixed_script("\u{0562}\u{0561}\u{0580}\u{0565}\u{0582}", "text");
        assert!(matches!(warning, MixedScriptDetection::Clean));
    }

    #[test]
    fn ignores_pure_cherokee() {
        let warning = detect_mixed_script("\u{13A0}\u{13A1}\u{13A2}", "text");
        assert!(matches!(warning, MixedScriptDetection::Clean));
    }
}
