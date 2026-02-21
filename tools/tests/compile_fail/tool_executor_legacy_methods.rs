use forge_tools::{
    ToolApprovalRequirement, ToolCtx, ToolEffectProfile, ToolError, ToolExecutor, ToolFut,
    ToolMetadata,
};
use forge_types::{Provider, ToolProviderScope, ToolVisibility};
use serde_json::Value;

struct LegacyExecutor;

impl ToolExecutor for LegacyExecutor {
    fn name(&self) -> &'static str {
        "Legacy"
    }

    fn description(&self) -> &'static str {
        "legacy test executor"
    }

    fn schema(&self) -> Value {
        serde_json::json!({})
    }

    fn effect_profile(&self, _args: &Value) -> ToolEffectProfile {
        ToolEffectProfile::Pure
    }

    fn approval_requirement(&self) -> ToolApprovalRequirement {
        ToolApprovalRequirement::Never
    }

    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            approval_requirement: ToolApprovalRequirement::Never,
            visibility: ToolVisibility::Visible,
            provider_scope: ToolProviderScope::AllProviders,
        }
    }

    fn is_side_effecting(&self, _args: &Value) -> bool {
        false
    }

    fn reads_user_data(&self, _args: &Value) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn is_hidden(&self) -> bool {
        false
    }

    fn target_provider(&self) -> Option<Provider> {
        None
    }

    fn approval_summary(&self, _args: &Value) -> Result<String, ToolError> {
        Ok(String::new())
    }

    fn execute<'a>(&'a self, _args: Value, _ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move { Ok(String::new()) })
    }
}

fn main() {}
