use crate::PredefinedModel;
use crate::app::{
    AppearanceEditorSnapshot, ContextEditorSnapshot, ModelEditorSnapshot, ToolsEditorSnapshot,
};
use crate::tools;
use crate::ui::UiOptions;
use forge_context::ModelName;

pub(crate) const APPEARANCE_SETTINGS_COUNT: usize = 4;
pub(crate) const CONTEXT_SETTINGS_COUNT: usize = 1;
pub(crate) const TOOLS_SETTINGS_COUNT: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppearanceSettingsEditor {
    pub(crate) baseline: UiOptions,
    pub(crate) draft: UiOptions,
    pub(crate) selected: usize,
}

impl AppearanceSettingsEditor {
    pub(crate) fn new(initial: UiOptions) -> Self {
        Self {
            baseline: initial,
            draft: initial,
            selected: 0,
        }
    }

    pub(crate) fn is_dirty(self) -> bool {
        self.draft != self.baseline
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelSettingsEditor {
    pub(crate) baseline: ModelName,
    pub(crate) draft: ModelName,
    pub(crate) selected: usize,
}

impl ModelSettingsEditor {
    pub(crate) fn new(initial: ModelName) -> Self {
        let selected = Self::index_for_model(&initial).unwrap_or(0);
        Self {
            baseline: initial.clone(),
            draft: initial,
            selected,
        }
    }

    pub(crate) fn update_draft_from_selected(&mut self) {
        if let Some(predefined) = PredefinedModel::all().get(self.selected) {
            self.draft = predefined.to_model_name();
        }
    }

    pub(crate) fn sync_selected_to_draft(&mut self) {
        if let Some(index) = Self::index_for_model(&self.draft) {
            self.selected = index;
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.draft != self.baseline
    }

    pub(crate) fn max_index() -> usize {
        PredefinedModel::all().len().saturating_sub(1)
    }

    pub(crate) fn index_for_model(model: &ModelName) -> Option<usize> {
        PredefinedModel::all()
            .iter()
            .position(|predefined| predefined.to_model_name() == *model)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolsSettingsEditor {
    pub(crate) baseline_approval_mode: tools::ApprovalMode,
    pub(crate) draft_approval_mode: tools::ApprovalMode,
    pub(crate) selected: usize,
}

impl ToolsSettingsEditor {
    pub(crate) fn new(initial_approval_mode: tools::ApprovalMode) -> Self {
        Self {
            baseline_approval_mode: initial_approval_mode,
            draft_approval_mode: initial_approval_mode,
            selected: 0,
        }
    }

    pub(crate) fn is_dirty(self) -> bool {
        self.draft_approval_mode != self.baseline_approval_mode
    }

    pub(crate) fn cycle_selected(&mut self) {
        if self.selected == 0 {
            self.draft_approval_mode = next_approval_mode(self.draft_approval_mode);
        }
    }
}

pub(crate) fn approval_mode_config_value(mode: tools::ApprovalMode) -> &'static str {
    match mode {
        tools::ApprovalMode::Permissive => "permissive",
        tools::ApprovalMode::Default => "default",
        tools::ApprovalMode::Strict => "strict",
    }
}

pub(crate) fn approval_mode_display(mode: tools::ApprovalMode) -> &'static str {
    match mode {
        tools::ApprovalMode::Permissive => "permissive",
        tools::ApprovalMode::Default => "default",
        tools::ApprovalMode::Strict => "strict",
    }
}

pub(crate) fn next_approval_mode(mode: tools::ApprovalMode) -> tools::ApprovalMode {
    match mode {
        tools::ApprovalMode::Permissive => tools::ApprovalMode::Default,
        tools::ApprovalMode::Default => tools::ApprovalMode::Strict,
        tools::ApprovalMode::Strict => tools::ApprovalMode::Permissive,
    }
}

pub(crate) fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

pub(crate) fn ui_options_display(options: UiOptions) -> String {
    let mut flags = Vec::new();
    if options.ascii_only {
        flags.push("ascii_only");
    }
    if options.high_contrast {
        flags.push("high_contrast");
    }
    if options.reduced_motion {
        flags.push("reduced_motion");
    }
    if options.show_thinking {
        flags.push("show_thinking");
    }
    if flags.is_empty() {
        return "default".to_string();
    }
    flags.join(", ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContextSettingsEditor {
    pub(crate) baseline_memory_enabled: bool,
    pub(crate) draft_memory_enabled: bool,
    pub(crate) selected: usize,
}

impl ContextSettingsEditor {
    pub(crate) fn new(initial_memory_enabled: bool) -> Self {
        Self {
            baseline_memory_enabled: initial_memory_enabled,
            draft_memory_enabled: initial_memory_enabled,
            selected: 0,
        }
    }

    pub(crate) fn is_dirty(self) -> bool {
        self.draft_memory_enabled != self.baseline_memory_enabled
    }
}

/// Mutually-exclusive settings editor state (IFA §9.2).
///
/// Only one detail editor may be active at a time.
#[derive(Debug, Clone)]
pub(crate) enum SettingsEditorState {
    Inactive,
    Model(ModelSettingsEditor),
    Tools(ToolsSettingsEditor),
    Context(ContextSettingsEditor),
    Appearance(AppearanceSettingsEditor),
}
