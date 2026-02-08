//! PhaseGate tool — forces generation boundaries between Gemini execution phases.
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
pub struct PhaseGateTool;

impl ToolExecutor for PhaseGateTool {
    fn name(&self) -> &'static str {
        "PhaseGate"
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

    fn is_side_effecting(&self) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn risk_level(&self) -> RiskLevel {
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
        Ok(format!("PhaseGate({phase})"))
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
    fn phase_gate_is_hidden() {
        let tool = PhaseGateTool;
        assert!(tool.is_hidden());
    }

    #[test]
    fn phase_gate_targets_gemini() {
        let tool = PhaseGateTool;
        assert_eq!(tool.target_provider(), Some(Provider::Gemini));
    }

    #[test]
    fn phase_gate_is_not_side_effecting() {
        let tool = PhaseGateTool;
        assert!(!tool.is_side_effecting());
    }

    #[test]
    fn phase_gate_does_not_require_approval() {
        let tool = PhaseGateTool;
        assert!(!tool.requires_approval());
    }

    #[test]
    fn phase_gate_risk_level_is_low() {
        let tool = PhaseGateTool;
        assert_eq!(tool.risk_level(), RiskLevel::Low);
    }
}
