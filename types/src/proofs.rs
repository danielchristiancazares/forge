//! Core proof types for validated content.
//!
//! These types enforce invariants at construction time. Once you hold a value,
//! you know it satisfies all required constraints.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// This type enforces the invariant that the contained string is never empty
/// (or whitespace-only) after trimming. Validation occurs at construction time,
/// so all operations on an existing `NonEmptyString` can assume the content is valid.
///
/// # Invariants
///
/// - Content is never empty after `trim()`
/// - Whitespace-only strings are rejected
///
/// # Serde
///
/// Serializes as a plain JSON string. Deserialization validates non-emptiness
/// and fails with an error if the string is empty or whitespace-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NonEmptyString(String);

#[derive(Debug, Error)]
#[error("message content must not be empty")]
pub struct EmptyStringError;

impl NonEmptyString {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyStringError> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(EmptyStringError)
        } else {
            Ok(Self(value))
        }
    }

    /// Build a `NonEmptyString` by concatenating a static prefix, separator, and existing content.
    #[must_use]
    pub fn prefixed(prefix: NonEmptyStaticStr, separator: &str, content: &NonEmptyString) -> Self {
        let mut value =
            String::with_capacity(prefix.as_str().len() + separator.len() + content.as_str().len());
        value.push_str(prefix.as_str());
        value.push_str(separator);
        value.push_str(content.as_str());
        Self(value)
    }

    #[must_use]
    pub fn append(mut self, suffix: impl AsRef<str>) -> Self {
        self.0.push_str(suffix.as_ref());
        Self(self.0)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl TryFrom<String> for NonEmptyString {
    type Error = EmptyStringError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for NonEmptyString {
    type Error = EmptyStringError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<NonEmptyString> for String {
    fn from(value: NonEmptyString) -> Self {
        value.0
    }
}

impl std::ops::Deref for NonEmptyString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for NonEmptyString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Like [`NonEmptyString`], but for `'static` string literals. Validates non-emptiness at
/// compile time via `const` assertion. Does not trim whitespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NonEmptyStaticStr(&'static str);

impl NonEmptyStaticStr {
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        assert!(!value.is_empty(), "NonEmptyStaticStr must not be empty");
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl TryFrom<NonEmptyStaticStr> for NonEmptyString {
    type Error = EmptyStringError;

    fn try_from(value: NonEmptyStaticStr) -> Result<Self, Self::Error> {
        Self::new(value.0)
    }
}

/// This type enforces the invariant that standalone `\r` characters are
/// normalized to `\n`. The normalization occurs at construction time
/// (single Authority Boundary per IFA-7).
///
/// # Invariant
///
/// - No standalone `\r` exists (only `\r\n` pairs permitted)
/// - Normalization: standalone `\r` → `\n`, `\r\n` preserved
///
/// # Security
///
/// Prevents log spoofing attacks where `\r` overwrites preceding content
/// when viewed in raw terminal contexts:
///
/// ```text
/// Attack: "File saved\rERROR: Permission denied"
/// Display: "ERROR: Permission denied" (overwrites "File saved")
/// Raw view: Hidden payload visible
/// ```
///
/// By normalizing at construction, we prevent this attack vector in
/// all persisted content (history, journals, logs).
///
/// # Performance
///
/// Uses a fast-path check: if no standalone `\r` is found, no allocation
/// is performed. Only strings containing attack vectors allocate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PersistableContent(String);

impl PersistableContent {
    /// Create persistable content by normalizing line endings.
    ///
    /// This is the ONLY constructor (Authority Boundary per IFA-7).
    /// Converts standalone `\r` to `\n` while preserving `\r\n` (Windows line endings).
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        let input = input.into();
        match Self::normalize_borrowed(&input) {
            Cow::Borrowed(_) => Self(input),
            Cow::Owned(normalized) => Self(normalized),
        }
    }

    #[must_use]
    pub fn normalize_borrowed(input: &str) -> Cow<'_, str> {
        if Self::needs_normalization(input) {
            Cow::Owned(Self::normalize(input))
        } else {
            Cow::Borrowed(input)
        }
    }

    fn needs_normalization(input: &str) -> bool {
        let bytes = input.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\r' && bytes.get(i + 1) != Some(&b'\n') {
                return true;
            }
        }
        false
    }

    fn normalize(input: &str) -> String {
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' {
                if chars.peek() == Some(&'\n') {
                    result.push('\r');
                    result.push(chars.next().unwrap());
                } else {
                    result.push('\n');
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl AsRef<str> for PersistableContent {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<String> for PersistableContent {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<PersistableContent> for String {
    fn from(value: PersistableContent) -> Self {
        value.0
    }
}

impl std::ops::Deref for PersistableContent {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl std::fmt::Display for PersistableContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) fn normalize_non_empty_for_persistence(value: &NonEmptyString) -> NonEmptyString {
    match PersistableContent::normalize_borrowed(value.as_str()) {
        Cow::Borrowed(_) => value.clone(),
        Cow::Owned(normalized) => {
            debug_assert!(!normalized.trim().is_empty());
            NonEmptyString(normalized)
        }
    }
}

pub(crate) fn normalize_string_for_persistence(value: &str) -> String {
    PersistableContent::normalize_borrowed(value).into_owned()
}
