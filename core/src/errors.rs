//! Error formatting for stream errors.
//!
//! This module handles parsing and formatting API errors into user-friendly messages.

use std::fmt::Write;

use serde_json::Value;

use forge_types::{NonEmptyStaticStr, NonEmptyString, Provider, truncate_with_ellipsis};

const STREAM_ERROR_BADGE: NonEmptyStaticStr = NonEmptyStaticStr::new("[Stream error]");

#[must_use]
pub fn split_api_error(raw: &str) -> Option<(String, String)> {
    let rest = raw.strip_prefix("API error ")?;
    let (status, body) = rest.split_once(": ")?;
    Some((status.trim().to_string(), body.trim().to_string()))
}

pub fn extract_error_message(raw: &str) -> Option<String> {
    let body = split_api_error(raw).map_or_else(|| raw.trim().to_string(), |(_, body)| body);
    let payload: Value = serde_json::from_str(&body).ok()?;
    payload
        .pointer("/error/message")
        .and_then(|value| value.as_str())
        .or_else(|| {
            payload
                .pointer("/response/error/message")
                .and_then(|value| value.as_str())
        })
        .or_else(|| payload.pointer("/message").and_then(|value| value.as_str()))
        .or_else(|| payload.as_str())
        .map(ToString::to_string)
}

#[must_use]
pub fn is_auth_error(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    let mentions_key =
        lower.contains("api key") || lower.contains("x-api-key") || lower.contains("authorization");
    let auth_words = lower.contains("invalid")
        || lower.contains("incorrect")
        || lower.contains("missing")
        || lower.contains("unauthorized")
        || lower.contains("not provided")
        || lower.contains("authentication");
    let has_code = lower.contains("401");

    lower.contains("invalid_api_key")
        || lower.contains("you must provide an api key")
        || (mentions_key && auth_words)
        || (mentions_key && has_code)
        || (has_code && lower.contains("unauthorized"))
}

/// Format a stream error into a user-friendly message.
#[must_use]
pub fn format_stream_error(provider: Provider, model: &str, err: &str) -> NonEmptyString {
    let trimmed = err.trim();
    let (status, body) =
        split_api_error(trimmed).unwrap_or_else(|| (String::new(), trimmed.to_string()));
    let extracted = extract_error_message(&body).unwrap_or_else(|| body.clone());
    let is_auth = is_auth_error(&extracted) || is_auth_error(trimmed) || is_auth_error(&status);

    if is_auth {
        let env_var = provider.env_var();
        let mut content = String::new();
        content.push_str(STREAM_ERROR_BADGE.as_str());
        content.push_str("\n\n");
        let _ = write!(
            content,
            "{} authentication failed for model {}.",
            provider.display_name(),
            model
        );
        content.push_str("\n\nFix:\n- Set ");
        content.push_str(env_var);
        let config_hint = forge_config::config_path().map_or_else(
            || "~/.forge/config.toml".to_string(),
            |p| p.display().to_string(),
        );
        let _ = write!(
            content,
            " (env) or add it to {config_hint} under [api_keys].\n- Then retry your message."
        );

        let detail = if status.trim().is_empty() {
            truncate_with_ellipsis(&extracted, 160)
        } else {
            status.trim().to_string()
        };
        if !detail.is_empty() {
            content.push_str("\n\nDetails: ");
            content.push_str(&detail);
        }

        return NonEmptyString::new(content).unwrap_or_else(|_| {
            NonEmptyString::try_from(STREAM_ERROR_BADGE)
                .expect("STREAM_ERROR_BADGE must be non-empty")
        });
    }

    let detail = if !extracted.trim().is_empty() {
        extracted.trim().to_string()
    } else if !trimmed.is_empty() {
        trimmed.to_string()
    } else {
        "unknown error".to_string()
    };
    let detail_short = truncate_with_ellipsis(&detail, 200);
    let mut content = String::new();
    content.push_str(STREAM_ERROR_BADGE.as_str());
    content.push_str("\n\n");
    if status.trim().is_empty() {
        content.push_str("Request failed.");
    } else {
        content.push_str("Request failed (");
        content.push_str(status.trim());
        content.push_str(").");
    }
    if !detail_short.is_empty() {
        content.push_str("\n\nDetails: ");
        content.push_str(&detail_short);
    }

    NonEmptyString::new(content).unwrap_or_else(|_| {
        NonEmptyString::try_from(STREAM_ERROR_BADGE).expect("STREAM_ERROR_BADGE must be non-empty")
    })
}
