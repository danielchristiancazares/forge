//! Recall tool for querying the Librarian's fact store.
//!
//! This tool allows the model to explicitly query its memory for facts
//! learned in previous conversations. Part of the Context Infinity system.

use forge_context::FactWithStaleness;
use serde::Deserialize;
use serde_json::json;

use super::{RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut, sanitize_output};

/// Tool for recalling facts from the Librarian.
#[derive(Debug, Default)]
pub struct RecallTool;

#[derive(Debug, Deserialize)]
struct RecallArgs {
    query: String,
}

impl ToolExecutor for RecallTool {
    fn name(&self) -> &'static str {
        "Recall"
    }

    fn description(&self) -> &'static str {
        "Query your memory for facts learned in previous conversations. \
         Use when you need context about decisions made, files discussed, \
         or constraints established in earlier turns."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to search for (e.g., 'TUI rendering', 'authentication decisions', 'file structure')"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn is_side_effecting(&self) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: RecallArgs =
            serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;
        Ok(format!("Recall facts about: {}", typed.query))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: RecallArgs =
                serde_json::from_value(args).map_err(|e| ToolError::BadArgs {
                    message: e.to_string(),
                })?;

            if typed.query.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "query must not be empty".to_string(),
                });
            }

            let Some(librarian_arc) = &ctx.librarian else {
                return Ok("Memory not available (Librarian disabled)".to_string());
            };

            // Search facts by keyword with staleness info
            let facts_with_staleness = {
                let librarian = librarian_arc.lock().await;
                match librarian.search_with_staleness(&typed.query) {
                    Ok(f) => f,
                    Err(e) => {
                        return Ok(format!("Memory search failed: {e}"));
                    }
                }
            };

            if facts_with_staleness.is_empty() {
                return Ok(format!("No facts found matching: {}", typed.query));
            }

            // Format output with staleness warnings
            let output = format_recall_output(&facts_with_staleness);
            Ok(sanitize_output(&output))
        })
    }
}

/// Format recalled facts for display with staleness warnings.
fn format_recall_output(facts: &[FactWithStaleness]) -> String {
    use std::fmt::Write;

    let mut output = String::from("## Recalled Context\n\n");
    let mut stale_count = 0;

    for fws in facts {
        let fact = &fws.fact.fact;
        let type_label = match fact.fact_type {
            forge_context::FactType::Entity => "📁",
            forge_context::FactType::Decision => "🔧",
            forge_context::FactType::Constraint => "⚠️",
            forge_context::FactType::CodeState => "📝",
            forge_context::FactType::Pinned => "📌",
        };

        if fws.is_stale() {
            stale_count += 1;
            // Show staleness warning with changed files
            let files: Vec<&str> = fws.stale_sources.iter().map(String::as_str).collect();
            let files_str = files.join(", ");
            let _ = writeln!(
                output,
                "⚠️ [stale: {} changed] {type_label} {}",
                files_str, fact.content
            );
        } else {
            let _ = writeln!(output, "{type_label} {}", fact.content);
        }
    }

    if stale_count > 0 {
        let _ = writeln!(
            output,
            "\nFound {} fact(s) ({} may be stale)",
            facts.len(),
            stale_count
        );
    } else {
        let _ = writeln!(output, "\nFound {} fact(s)", facts.len());
    }
    output
}
