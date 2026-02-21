use crate::PredefinedModel;

use crate::tools;
use crate::ui::UiOptions;
use forge_types::ModelName;
use forge_types::ui::{AsciiOnly, HighContrast, ReducedMotion, ShowThinking};
use tools::ApprovalMode;

pub(crate) const APPEARANCE_SETTINGS_COUNT: usize = 4;
pub(crate) const CONTEXT_SETTINGS_COUNT: usize = 1;
pub(crate) const TOOLS_SETTINGS_COUNT: usize = 1;

/// Core domain representation of an editor's unsaved state (IFA §9 Structurally encoded state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorState<T> {
    Clean {
        baseline: T,
        selected: usize,
    },
    Unsaved {
        baseline: T,
        draft: T,
        selected: usize,
    },
}

impl<T: Clone + PartialEq> EditorState<T> {
    pub(crate) fn new(baseline: T, selected: usize) -> Self {
        Self::Clean { baseline, selected }
    }

    pub(crate) fn with_draft(baseline: T, draft: T, selected: usize) -> Self {
        if baseline == draft {
            Self::Clean { baseline, selected }
        } else {
            Self::Unsaved {
                baseline,
                draft,
                selected,
            }
        }
    }

    pub(crate) fn baseline(&self) -> &T {
        match self {
            Self::Clean { baseline, .. } | Self::Unsaved { baseline, .. } => baseline,
        }
    }

    pub(crate) fn draft(&self) -> &T {
        match self {
            Self::Clean { baseline, .. } => baseline,
            Self::Unsaved { draft, .. } => draft,
        }
    }

    pub(crate) fn selected(&self) -> usize {
        match self {
            Self::Clean { selected, .. } | Self::Unsaved { selected, .. } => *selected,
        }
    }

    pub(crate) fn set_selected(&mut self, new_selected: usize) {
        match self {
            Self::Clean { selected, .. } | Self::Unsaved { selected, .. } => {
                *selected = new_selected;
            }
        }
    }

    pub(crate) fn is_unsaved(&self) -> bool {
        matches!(self, Self::Unsaved { .. })
    }
}

impl EditorState<UiOptions> {
    pub(crate) fn toggle_selected(&mut self) {
        let selected = self.selected();
        let mut draft = *self.draft();
        match selected {
            0 => {
                draft.ascii_only = match draft.ascii_only {
                    AsciiOnly::Disabled => AsciiOnly::Enabled,
                    AsciiOnly::Enabled => AsciiOnly::Disabled,
                };
            }
            1 => {
                draft.high_contrast = match draft.high_contrast {
                    HighContrast::Disabled => HighContrast::Enabled,
                    HighContrast::Enabled => HighContrast::Disabled,
                };
            }
            2 => {
                draft.reduced_motion = match draft.reduced_motion {
                    ReducedMotion::Disabled => ReducedMotion::Enabled,
                    ReducedMotion::Enabled => ReducedMotion::Disabled,
                };
            }
            3 => {
                draft.show_thinking = match draft.show_thinking {
                    ShowThinking::Disabled => ShowThinking::Enabled,
                    ShowThinking::Enabled => ShowThinking::Disabled,
                };
            }
            _ => {}
        }
        *self = Self::with_draft(*self.baseline(), draft, selected);
    }
}

impl EditorState<ModelName> {
    pub(crate) fn from_model(initial: ModelName) -> Self {
        let selected = Self::index_for_model(&initial).unwrap_or(0);
        Self::Clean {
            baseline: initial,
            selected,
        }
    }

    pub(crate) fn update_draft_from_selected(&mut self) {
        let selected = self.selected();
        if let Some(predefined) = PredefinedModel::all().get(selected) {
            *self = Self::with_draft(
                self.baseline().clone(),
                predefined.to_model_name(),
                selected,
            );
        }
    }

    pub(crate) fn sync_selected_to_draft(&mut self) {
        let draft = self.draft().clone();
        if let Some(index) = Self::index_for_model(&draft) {
            *self = Self::with_draft(self.baseline().clone(), draft, index);
        }
    }

    pub(crate) fn max_model_index() -> usize {
        PredefinedModel::all().len().saturating_sub(1)
    }

    pub(crate) fn index_for_model(model: &ModelName) -> Option<usize> {
        PredefinedModel::all()
            .iter()
            .position(|predefined| predefined.to_model_name() == *model)
    }
}

impl EditorState<ApprovalMode> {
    pub(crate) fn cycle_selected(&mut self) {
        let selected = self.selected();
        if selected == 0 {
            let next_draft = next_approval_mode(*self.draft());
            *self = Self::with_draft(*self.baseline(), next_draft, selected);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryState {
    Enabled,
    Disabled,
}

impl MemoryState {
    pub(crate) fn from_bool(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    pub(crate) fn as_bool(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl EditorState<MemoryState> {
    pub(crate) fn cycle_selected(&mut self) {
        let selected = self.selected();
        if selected == 0 {
            let draft = *self.draft();
            let next_draft = match draft {
                MemoryState::Enabled => MemoryState::Disabled,
                MemoryState::Disabled => MemoryState::Enabled,
            };
            *self = Self::with_draft(*self.baseline(), next_draft, selected);
        }
    }
}

pub(crate) fn approval_mode_config_value(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Permissive => "permissive",
        ApprovalMode::Balanced => "balanced",
        ApprovalMode::Strict => "strict",
    }
}

pub(crate) fn approval_mode_display(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Permissive => "permissive",
        ApprovalMode::Balanced => "balanced",
        ApprovalMode::Strict => "strict",
    }
}

pub(crate) fn next_approval_mode(mode: ApprovalMode) -> ApprovalMode {
    match mode {
        ApprovalMode::Permissive => ApprovalMode::Balanced,
        ApprovalMode::Balanced => ApprovalMode::Strict,
        ApprovalMode::Strict => ApprovalMode::Permissive,
    }
}

pub(crate) fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

pub(crate) fn ui_options_display(options: UiOptions) -> String {
    let mut flags = Vec::new();
    if matches!(options.ascii_only, AsciiOnly::Enabled) {
        flags.push("ascii_only");
    }
    if matches!(options.high_contrast, HighContrast::Enabled) {
        flags.push("high_contrast");
    }
    if matches!(options.reduced_motion, ReducedMotion::Enabled) {
        flags.push("reduced_motion");
    }
    if matches!(options.show_thinking, ShowThinking::Enabled) {
        flags.push("show_thinking");
    }
    if flags.is_empty() {
        return "default".to_string();
    }
    flags.join(", ")
}

/// Mutually-exclusive settings editor state (IFA §9.2).
///
/// Only one detail editor may be active at a time.
#[derive(Debug, Clone)]
pub(crate) enum SettingsEditorState {
    Inactive,
    Model(EditorState<ModelName>),
    Tools(EditorState<ApprovalMode>),
    Context(EditorState<MemoryState>),
    Appearance(EditorState<UiOptions>),
}
