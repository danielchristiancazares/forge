//! GeminiGate tool — forces generation boundaries between Gemini execution phases.
//!
//! Gemini 3 Pro collapses multi-phase execution into a single generation pass.
//! This no-op tool forces the model to stop generating and re-enter inference
//! at each phase transition, preventing verification spirals.
//!
//! The tool is hidden from the UI and only sent to the Gemini provider.

use forge_types::Provider;
use serde_json::{Value, json};

use super::{RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut};

#[derive(Debug)]
pub struct GeminiGateTool;

impl ToolExecutor for GeminiGateTool {
    fn name(&self) -> &'static str {
        "GeminiGate"
    }

    fn description(&self) -> &'static str {
        "Signal transition between execution phases. Call before entering each new phase."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "phase": {
                    "type": "integer",
                    "description": "The phase number to enter (2 or 3)"
                }
            },
            "required": ["phase"]
        })
    }

    fn is_side_effecting(&self, _args: &Value) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn risk_level(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Low
    }

    fn is_hidden(&self) -> bool {
        true
    }

    fn target_provider(&self) -> Option<Provider> {
        Some(Provider::Gemini)
    }

    fn approval_summary(&self, args: &Value) -> Result<String, ToolError> {
        let phase = args.get("phase").and_then(Value::as_i64).unwrap_or(0);
        Ok(format!("GeminiGate({phase})"))
    }

    fn execute<'a>(&'a self, args: Value, _ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let _phase: i64 =
                args.get("phase")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| ToolError::BadArgs {
                        message: "phase must be an integer".to_string(),
                    })?;
            Ok("ok".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_gate_is_hidden() {
        let tool = GeminiGateTool;
        assert!(tool.is_hidden());
    }

    #[test]
    fn gemini_gate_targets_gemini() {
        let tool = GeminiGateTool;
        assert_eq!(tool.target_provider(), Some(Provider::Gemini));
    }

    #[test]
    fn gemini_gate_is_not_side_effecting() {
        let tool = GeminiGateTool;
        assert!(!tool.is_side_effecting(&serde_json::json!({})));
    }

    #[test]
    fn gemini_gate_does_not_require_approval() {
        let tool = GeminiGateTool;
        assert!(!tool.requires_approval());
    }

    #[test]
    fn gemini_gate_risk_level_is_low() {
        let tool = GeminiGateTool;
        assert_eq!(tool.risk_level(&serde_json::json!({})), RiskLevel::Low);
    }
}
