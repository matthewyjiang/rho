//! Overlay single-line text input for config keys and agent fields.

use ratatui::text::Line;

use super::{
    config_editor::ConfigTextKey,
    line_editor::LineEditor,
    picker::UiPicker,
    render::{styled_line, truncate_one_line, LineFill},
    theme::Theme,
};

/// Which overlay owns a [`TextInput`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TextInputTarget {
    ConfigApiKey(ConfigTextKey),
    AgentField(AgentField),
    /// One step of the custom-host `/login` wizard, which owns its own state.
    CustomHost(super::custom_provider_login::CustomHostStep),
}

/// Editable agent frontmatter fields that use the shared line editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentField {
    Description,
    Model,
    Provider,
    Tools,
}

impl AgentField {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Description => "description",
            Self::Model => "model",
            Self::Provider => "provider",
            Self::Tools => "tools",
        }
    }

    /// Stable picker item value used to restore position after a text edit.
    pub(super) const fn value(self) -> &'static str {
        match self {
            Self::Description => "agent_field:description",
            Self::Model => "agent_field:model",
            Self::Provider => "agent_field:provider",
            Self::Tools => "agent_field:tools",
        }
    }
}

/// Shared unmasked/masked single-line overlay input.
#[derive(Clone, Debug)]
pub(super) struct TextInput {
    pub(super) target: TextInputTarget,
    pub(super) editor: LineEditor,
}

impl TextInput {
    pub(super) fn config_api_key(key: ConfigTextKey, value: Option<String>) -> Self {
        Self {
            target: TextInputTarget::ConfigApiKey(key),
            editor: LineEditor::new(value.unwrap_or_default()),
        }
    }

    pub(super) fn agent_field(field: AgentField, value: impl Into<String>) -> Self {
        Self {
            target: TextInputTarget::AgentField(field),
            editor: LineEditor::new(value),
        }
    }

    /// A wizard step, seeded with `value` so a rejected entry is not discarded.
    pub(super) fn custom_host(
        step: super::custom_provider_login::CustomHostStep,
        value: impl Into<String>,
    ) -> Self {
        Self {
            target: TextInputTarget::CustomHost(step),
            editor: LineEditor::new(value),
        }
    }

    pub(super) fn with_return_picker(mut self, picker: UiPicker) -> Self {
        self.editor = self.editor.with_return_picker(picker);
        self
    }

    pub(super) fn take_return_picker(&mut self) -> Option<UiPicker> {
        self.editor.take_return_picker()
    }

    pub(super) fn is_masked(&self) -> bool {
        matches!(self.target, TextInputTarget::ConfigApiKey(_))
    }

    pub(super) fn label(&self) -> &str {
        match &self.target {
            TextInputTarget::ConfigApiKey(key) => key.label(),
            TextInputTarget::AgentField(field) => field.label(),
            TextInputTarget::CustomHost(step) => step.label(),
        }
    }

    /// Footer verb for Enter. A wizard step advances; everything else saves.
    fn confirm_verb(&self) -> &'static str {
        match &self.target {
            TextInputTarget::CustomHost(_) => "Enter continue",
            TextInputTarget::ConfigApiKey(_) | TextInputTarget::AgentField(_) => "Enter save",
        }
    }

    pub(super) fn display_value(&self) -> String {
        if self.is_masked() {
            "•".repeat(self.editor.value.chars().count())
        } else {
            self.editor.value.clone()
        }
    }
}

pub(super) fn text_input_lines(input: &TextInput, width: usize) -> Vec<Line<'static>> {
    vec![
        styled_line(
            truncate_one_line(
                &format!(
                    "edit {}  {}",
                    input.label(),
                    super::composer_chrome::join_footer_parts([input.confirm_verb(), "Esc cancel"])
                ),
                width,
            ),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&input.display_value(), width),
            width,
            Theme::text(),
            LineFill::Natural,
        ),
    ]
}
