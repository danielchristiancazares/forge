use ratatui::{style::Style, text::Span};

use forge_engine::CompletedTurnUsage;
use forge_types::ApiUsagePresence;

use crate::theme::Palette;

/// Split text into styled spans, highlighting `@path` file references in crystalBlue.
pub(crate) fn highlight_file_refs<'a>(text: &str, palette: &Palette) -> Vec<Span<'a>> {
    let normal = Style::default().fg(palette.text_primary);
    let file_ref = Style::default().fg(palette.blue);

    let mut spans = Vec::new();
    let mut rest = text;

    while let Some(at_pos) = rest.find('@') {
        if at_pos > 0 {
            spans.push(Span::styled(rest[..at_pos].to_string(), normal));
        }

        let after_at = &rest[at_pos + 1..];
        let end = after_at
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after_at.len());

        if end == 0 {
            spans.push(Span::styled("@".to_string(), normal));
            rest = after_at;
            continue;
        }

        let token = &rest[at_pos..at_pos + 1 + end];
        spans.push(Span::styled(token.to_string(), file_ref));
        rest = &after_at[end..];
    }

    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), normal));
    }

    spans
}

pub(crate) fn format_token_count(value: u32) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f32 / 1_000_000.0)
    } else if value >= 1000 {
        format!("{:.1}k", value as f32 / 1000.0)
    } else {
        value.to_string()
    }
}

pub(crate) fn format_api_usage(usage: &CompletedTurnUsage) -> String {
    let CompletedTurnUsage::Available(usage) = usage else {
        return String::new();
    };
    if matches!(usage.total.presence(), ApiUsagePresence::Unused) {
        return String::new();
    }

    let input = usage.total.input_tokens;
    let output = usage.total.output_tokens;
    let cache_pct = usage.total.cache_hit_percentage();

    let fmt_tokens = |n: u32| -> String {
        if n >= 10_000 {
            format!("{}k", n / 1000)
        } else if n >= 1_000 {
            format!("{:.1}k", n as f64 / 1000.0)
        } else {
            n.to_string()
        }
    };

    let input_str = fmt_tokens(input);
    let output_str = fmt_tokens(output);

    if cache_pct > 0.5 {
        format!("Tokens {input_str} in / {output_str} out ({cache_pct:.0}% cached)")
    } else {
        format!("Tokens {input_str} in / {output_str} out")
    }
}
