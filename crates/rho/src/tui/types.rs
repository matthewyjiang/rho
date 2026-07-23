//! Core TUI value types shared across interactive modules.

use std::time::{Duration, Instant};

use ratatui::{
    style::{Modifier, Style},
    text::Line,
};
use rho_providers::model::{
    catalog::{LoginTarget, ModelSelection},
    ContextUsage, ModelUsage,
};
use rho_tools::tool::ToolDisplayStyle;

use super::{
    approval::ApprovalComposer,
    commands::{self, CommandSpec},
    config_editor::{ConfigNumberInput, ConfigTextInput},
    feed_image::FeedImage,
    info_command,
    inline_choice::InlineChoiceModal,
    inline_shell::InlineShellMode,
    login::SecretInput,
    markdown::CodeFenceState,
    picker::UiPicker,
    prompt_turn::FailedTurn,
    questionnaire::QuestionnaireComposer,
    stream::AppendOnlyStream,
    theme::Theme,
    usage_cost::UsageCostTracker,
};

#[cfg(test)]
pub(super) struct ActiveFrame {
    pub(in crate::tui) lines: Vec<Line<'static>>,
}

pub(super) struct LiveStreamPreview {
    pub(in crate::tui) kind: StreamKind,
    pub(in crate::tui) text: String,
    pub(in crate::tui) include_leading_blank: bool,
}

pub(super) struct SessionHeaderCache {
    pub(in crate::tui) width: usize,
    pub(in crate::tui) update_notice: Option<String>,
    pub(in crate::tui) lines: Vec<Line<'static>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct InteractiveModelSelection {
    pub(in crate::tui) selection: ModelSelection,
    pub(in crate::tui) alias: Option<String>,
}

/// Live assistant/reasoning stream UI state owned by [`super::App`].
#[derive(Default)]
pub(super) struct StreamUi {
    pub(in crate::tui) assistant_stream: AppendOnlyStream,
    pub(in crate::tui) assistant_stream_code_fence: CodeFenceState,
    pub(in crate::tui) reasoning_stream: AppendOnlyStream,
    pub(in crate::tui) reasoning_stream_code_fence: CodeFenceState,
    pub(in crate::tui) current_stream_kind: Option<StreamKind>,
    pub(in crate::tui) stream_preview_deadline: Option<Instant>,
    pub(in crate::tui) live_stream_preview: Option<LiveStreamPreview>,
}

impl StreamUi {
    pub(super) fn reset(&mut self) {
        self.assistant_stream.reset();
        self.assistant_stream_code_fence = CodeFenceState::default();
        self.reasoning_stream.reset();
        self.reasoning_stream_code_fence = CodeFenceState::default();
        self.current_stream_kind = None;
        self.stream_preview_deadline = None;
        self.live_stream_preview = None;
    }

    pub(super) fn loading_streams_active(&self) -> bool {
        !self.assistant_stream.is_empty() || !self.reasoning_stream.is_empty()
    }
}

/// Cumulative and in-flight usage snapshots shown by the TUI.
#[derive(Default)]
pub(super) struct UsageUi {
    pub(in crate::tui) cumulative_usage: Option<ModelUsage>,
    pub(in crate::tui) usage_cost_tracker: UsageCostTracker,
    // SDK usage updates are cumulative within a run. These snapshots let the TUI
    // replace active usage while preserving totals from prior runs and steps.
    pub(in crate::tui) usage_before_current_run: Option<ModelUsage>,
    pub(in crate::tui) usage_before_current_step: Option<ModelUsage>,
    pub(in crate::tui) usage_before_current_attempt: Option<ModelUsage>,
    pub(in crate::tui) current_run_usage: Option<ModelUsage>,
    pub(in crate::tui) latest_usage: Option<ModelUsage>,
    pub(in crate::tui) current_context: Option<ContextUsage>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum InputSubmissionMode {
    #[default]
    ParseCommands,
    Prompt,
}

#[derive(Debug, Default)]
pub(super) enum ComposerMode {
    #[default]
    Input,
    Picker(UiPicker),
    SecretInput(SecretInput),
    ConfigNumberInput(ConfigNumberInput),
    ConfigTextInput(ConfigTextInput),
    OAuthPending(LoginTarget),
    InlineChoice(InlineChoiceModal),
    Questionnaire(QuestionnaireComposer),
    Approval(ApprovalComposer),
}

impl ComposerMode {
    pub(super) fn blocks_auto_continue(&self) -> bool {
        match self {
            Self::InlineChoice(modal) => modal.blocks_auto_continue(),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PasteSegment {
    pub(in crate::tui) start: usize,
    pub(in crate::tui) marker_len: usize,
    pub(in crate::tui) content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QueuedPrompt {
    pub(in crate::tui) prompt: String,
    pub(in crate::tui) display_prompt: String,
    pub(in crate::tui) paste_segments: Vec<PasteSegment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InputDraft {
    pub(in crate::tui) input: String,
    pub(in crate::tui) paste_segments: Vec<PasteSegment>,
    pub(in crate::tui) submission_mode: InputSubmissionMode,
    pub(in crate::tui) shell_mode: Option<InlineShellMode>,
}

#[derive(Clone, Debug)]
pub(super) struct FileMatchCache {
    pub(in crate::tui) query: String,
    pub(in crate::tui) matches: std::sync::Arc<Vec<String>>,
    pub(in crate::tui) refreshed_at: Instant,
}

/// Discovered skills reused across command palette queries, so typing a slash
/// command does not re-walk skill directories on every keystroke.
pub(super) struct SkillMatchCache {
    pub(in crate::tui) skills: std::sync::Arc<Vec<crate::skills::Skill>>,
    pub(in crate::tui) refreshed_at: Instant,
}

impl From<&str> for QueuedPrompt {
    fn from(prompt: &str) -> Self {
        Self {
            prompt: prompt.to_string(),
            display_prompt: prompt.to_string(),
            paste_segments: Vec::new(),
        }
    }
}

impl PasteSegment {
    pub(super) fn end(&self) -> usize {
        self.start + self.marker_len
    }
}

#[derive(Debug)]
pub(super) struct SessionTitleResult {
    pub(in crate::tui) session_id: String,
    pub(in crate::tui) title: anyhow::Result<String>,
}

#[derive(Clone, Debug)]
pub(super) struct CommandChoice {
    pub(in crate::tui) name: String,
    pub(in crate::tui) usage: String,
    pub(in crate::tui) description: String,
    pub(in crate::tui) kind: CommandChoiceKind,
}

#[derive(Debug, PartialEq)]
pub(super) enum TurnOutcome {
    Completed,
    Interrupted,
    /// User cancelled interactive work such as a questionnaire.
    Cancelled,
    Failed(FailedTurn),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TurnOutcomeKind {
    Completed,
    Interrupted,
    Cancelled,
    Failed,
}

impl TurnOutcome {
    pub(super) fn kind(&self) -> TurnOutcomeKind {
        match self {
            Self::Completed => TurnOutcomeKind::Completed,
            Self::Interrupted => TurnOutcomeKind::Interrupted,
            Self::Cancelled => TurnOutcomeKind::Cancelled,
            Self::Failed(_) => TurnOutcomeKind::Failed,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum HistoryScroll {
    #[default]
    Bottom,
    Manual {
        top_line: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommandChoiceKind {
    Builtin(&'static CommandSpec),
    BuiltinArgument(&'static commands::CommandArgumentChoice),
    PromptTemplate(String),
    Skill,
}

#[derive(Clone, Debug)]
pub(super) struct ToolEntry {
    pub(in crate::tui) state: ToolEntryState,
    pub(in crate::tui) display_lines: Vec<String>,
    pub(in crate::tui) expanded: bool,
    pub(in crate::tui) image: Option<FeedImage>,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ToolEntryState {
    Running,
    Finished {
        ok: bool,
        display_style: ToolDisplayStyle,
    },
}

#[derive(Clone, Debug)]
pub(super) enum Entry {
    User(String),
    Assistant(String),
    Reasoning(ReasoningEntry),
    Tool(ToolEntry),
    Notice(String),
    RuntimeInfo(Box<info_command::RuntimeInfo>),
    UsageLimits(crate::usage_limits::ProviderLimits),
    Error(String),
}

/// Streamed reasoning text plus optional post-phase thought duration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReasoningEntry {
    pub(in crate::tui) text: String,
    pub(in crate::tui) thought_for: Option<Duration>,
}

impl ReasoningEntry {
    pub(super) fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            thought_for: None,
        }
    }

    pub(super) fn summary_only(thought_for: Duration) -> Self {
        Self {
            text: String::new(),
            thought_for: Some(thought_for),
        }
    }
}

impl From<&str> for ReasoningEntry {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for ReasoningEntry {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl Entry {
    pub(super) fn is_provider_replaceable(&self) -> bool {
        matches!(self, Self::Assistant(_) | Self::Reasoning(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamKind {
    Assistant,
    Reasoning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PasteBurstKey {
    Char(char),
    Enter,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FinalAnswerDelta<'a> {
    None,
    Append(&'a str),
    Mismatch,
}

impl StreamKind {
    pub(super) fn style(self) -> Style {
        match self {
            Self::Assistant => Theme::text(),
            Self::Reasoning => Theme::dim().add_modifier(Modifier::DIM),
        }
    }

    pub(super) fn entry(self, text: String) -> Entry {
        match self {
            Self::Assistant => Entry::Assistant(text),
            Self::Reasoning => Entry::Reasoning(ReasoningEntry::new(text)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamControl {
    Continue,
    Interrupt,
    Resize,
    ApprovalResolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HerdrUserWait {
    Approval,
    Questionnaire,
}

impl HerdrUserWait {
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::Approval => "waiting for approval",
            Self::Questionnaire => "waiting for your answers",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunningInputMode {
    Turn,
    Compacting,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum HistoryDirection {
    Previous,
    Next,
}
