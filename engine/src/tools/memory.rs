//! Memory tool for pinning facts via the Librarian's fact store.

use serde::Deserialize;
use serde_json::json;

use super::{RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut, parse_args};

/// Tool for storing facts in the Librarian's memory.
#[derive(Debug, Default)]
pub struct MemoryTool;

#[derive(Debug, Deserialize)]
struct MemoryArgs {
    content: String,
    entities: Vec<String>,
}

impl ToolExecutor for MemoryTool {
    fn name(&self) -> &'static str {
        "Memory"
    }

    fn description(&self) -> &'static str {
        "Store an important fact, decision, constraint, or user preference for future recall. \
         Use when you learn something worth remembering across conversations — architectural \
         decisions, user preferences, project constraints, or key code facts."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The fact to memorize (e.g., 'User prefers tabs over spaces', 'Auth uses JWT with RS256')"
                },
                "entities": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Related entities for retrieval (e.g., file paths, module names, concept names)"
                }
            },
            "required": ["content", "entities"],
            "additionalProperties": false
        })
    }

    fn is_side_effecting(&self) -> bool {
        true
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: MemoryArgs = parse_args(args)?;
        Ok(format!("Memorize: {}", typed.content))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: MemoryArgs = parse_args(&args)?;

            if typed.content.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "content must not be empty".to_string(),
                });
            }

            let sanitized_entities: Vec<String> = typed
                .entities
                .into_iter()
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect();

            let Some(librarian_arc) = &ctx.librarian else {
                return Ok("Memory not available (Librarian disabled)".to_string());
            };

            let mut librarian = librarian_arc.lock().await;
            match librarian.pin_fact(&typed.content, &sanitized_entities) {
                Ok(()) => {
                    if sanitized_entities.is_empty() {
                        Ok(format!("Memorized: {}", typed.content))
                    } else {
                        Ok(format!(
                            "Memorized: {} [entities: {}]",
                            typed.content,
                            sanitized_entities.join(", ")
                        ))
                    }
                }
                Err(e) => Ok(format!("Failed to memorize: {e}")),
            }
        })
    }
}
