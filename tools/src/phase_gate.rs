//! GeminiGate tool — forces generation boundaries between Gemini execution phases.
//!
//! Gemini 3 Pro collapses multi-phase execution into a single generation pass.
//! This tool forces the model to stop generating and re-enter inference at each
//! phase transition. Each transition returns a phase-specific checklist that the
//! model must answer, forcing self-interrogation via token generation.
//!
//! The tool is hidden from the UI and only sent to the Gemini provider.

use forge_types::Provider;
use serde_json::{Value, json};

use super::{RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut};

const PHASE_2_CHECKLIST: &str = "\
Phase 1 complete. Answer each item before proceeding.
1. Task type: [conversation | question | review | code change]
2. Candidate files (list every file path):
3. Claims to verify (list each):";

const PHASE_3_CHECKLIST: &str = "\
Phase 2 complete. Answer each item before proceeding.
1. Verification result: [Pass | Fail — reason]
2. Files read (list every file path you read):
3. Paths confirmed to exist (list each):
4. Dangerous commands: [none found | list each]";

const PHASE_4_CHECKLIST: &str = "\
Ready for Phase 4. Answer each item before generating output.
1. Deliverable type: [answer | findings | patch]
2. Verified evidence (list file:line references):
3. Unverified claims: [none | list each]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Verification,
    Falsification,
    Execution,
}

impl Phase {
    fn from_number(n: i64) -> Result<Self, ToolError> {
        match n {
            2 => Ok(Self::Verification),
            3 => Ok(Self::Falsification),
            4 => Ok(Self::Execution),
            _ => Err(ToolError::BadArgs {
                message: format!("invalid phase {n}: must be 2, 3, or 4"),
            }),
        }
    }

    fn checklist(self) -> &'static str {
        match self {
            Self::Verification => PHASE_2_CHECKLIST,
            Self::Falsification => PHASE_3_CHECKLIST,
            Self::Execution => PHASE_4_CHECKLIST,
        }
    }
}

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
                    "description": "The phase number to enter (2, 3, or 4)"
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
            let n =
                args.get("phase")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| ToolError::BadArgs {
                        message: "phase must be an integer".to_string(),
                    })?;
            let phase = Phase::from_number(n)?;
            Ok(phase.checklist().to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{GeminiGateTool, Phase, Provider, RiskLevel, ToolExecutor, json};

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
        assert!(!tool.is_side_effecting(&json!({})));
    }

    #[test]
    fn gemini_gate_does_not_require_approval() {
        let tool = GeminiGateTool;
        assert!(!tool.requires_approval());
    }

    #[test]
    fn gemini_gate_risk_level_is_low() {
        let tool = GeminiGateTool;
        assert_eq!(tool.risk_level(&json!({})), RiskLevel::Low);
    }

    #[test]
    fn phase_from_number_valid() {
        assert_eq!(Phase::from_number(2).unwrap(), Phase::Verification);
        assert_eq!(Phase::from_number(3).unwrap(), Phase::Falsification);
        assert_eq!(Phase::from_number(4).unwrap(), Phase::Execution);
    }

    #[test]
    fn phase_from_number_invalid() {
        assert!(Phase::from_number(0).is_err());
        assert!(Phase::from_number(1).is_err());
        assert!(Phase::from_number(5).is_err());
    }

    #[test]
    fn phase_checklists_contain_expected_content() {
        assert!(Phase::Verification.checklist().contains("Task type"));
        assert!(Phase::Verification.checklist().contains("Candidate files"));
        assert!(
            Phase::Falsification
                .checklist()
                .contains("Verification result")
        );
        assert!(Phase::Falsification.checklist().contains("Files read"));
        assert!(Phase::Execution.checklist().contains("Deliverable type"));
        assert!(Phase::Execution.checklist().contains("Verified evidence"));
    }
}
