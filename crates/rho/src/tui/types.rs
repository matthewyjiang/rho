//! Core TUI value types shared across interactive modules.

use std::time::{Duration, Instant};

use super::{
    approval::ApprovalComposer,
    chat_media::ChatMedia,
    commands::{self, CommandSpec},
    config_editor::ConfigNumberInput,
    feed_image::FeedImage,
    info_command,
    inline_choice::InlineChoiceModal,
    inline_shell::InlineShellMode,
    limits_command,
    login::SecretInput,
    markdown::CodeFenceState,
    picker::UiPicker,
    prompt_turn::FailedTurn,
    questionnaire::QuestionnaireComposer,
    stream::AppendOnlyStream,
    stream_pace::StreamPacer,
    theme::Theme,
    usage_cost::{AttemptAwareRunUsage, UsageCostTracker},
};
use ratatui::{
    style::{Modifier, Style},
    text::Line,
};
use rho_providers::model::{
    catalog::{LoginTarget, ModelSelection},
    ContextUsage, ModelUsage,
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

/// Keyed paint of the live assistant/reasoning preview.
///
/// Dropped when preview identity or committed fence state changes. Hits skip
/// markdown + highlighter clone when width and theme generation match.
#[derive(Debug)]
pub(in crate::tui) struct StreamPreviewRenderCache {
    pub(in crate::tui) width: usize,
    pub(in crate::tui) theme_generation: u64,
    pub(in crate::tui) lines: Vec<Line<'static>>,
}

pub(super) struct SessionHeaderCache {
    pub(in crate::tui) width: usize,
    pub(in crate::tui) update_notice: Option<String>,
    pub(in crate::tui) setup: super::first_run::SetupState,
    /// Rebuild styled header lines when the active theme changes.
    pub(in crate::tui) theme_generation: u64,
    pub(in crate::tui) lines: Vec<Line<'static>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Next opportunity to release held text and refresh the partial preview.
    pub(in crate::tui) stream_tick_deadline: Option<Instant>,
    pub(in crate::tui) live_stream_preview: Option<LiveStreamPreview>,
    pub(in crate::tui) preview_render_cache: Option<StreamPreviewRenderCache>,
    #[cfg(test)]
    pub(in crate::tui) preview_paints: u32,
    /// Provider text waiting to be released into the active stream.
    pub(in crate::tui) hold: String,
    pub(in crate::tui) pacer: StreamPacer,
}

impl StreamUi {
    pub(super) fn reset(&mut self) {
        self.assistant_stream.reset();
        self.assistant_stream_code_fence = CodeFenceState::default();
        self.reasoning_stream.reset();
        self.reasoning_stream_code_fence = CodeFenceState::default();
        self.current_stream_kind = None;
        self.stream_tick_deadline = None;
        self.set_live_preview(None);
        self.invalidate_preview_cache();
        self.hold.clear();
        self.pacer.reset();
    }

    /// Replace the live preview. Drops the paint cache when identity changes.
    pub(super) fn set_live_preview(&mut self, preview: Option<LiveStreamPreview>) {
        let changed = match (&self.live_stream_preview, &preview) {
            (None, None) => false,
            (Some(current), Some(next)) => {
                current.kind != next.kind
                    || current.text != next.text
                    || current.include_leading_blank != next.include_leading_blank
            }
            (None, Some(_)) | (Some(_), None) => true,
        };
        if changed {
            self.live_stream_preview = preview;
            self.preview_render_cache = None;
        }
    }

    pub(super) fn invalidate_preview_cache(&mut self) {
        self.preview_render_cache = None;
    }

    pub(super) fn loading_streams_active(&self) -> bool {
        !self.hold.is_empty()
            || !self.assistant_stream.is_empty()
            || !self.reasoning_stream.is_empty()
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
    pub(in crate::tui) run_usage: AttemptAwareRunUsage,
    pub(in crate::tui) latest_usage: Option<ModelUsage>,
    pub(in crate::tui) model_performance: super::model_performance::ModelPerformanceTracker,
    pub(in crate::tui) current_context: Option<ContextUsage>,
    /// In-flight stream estimate while the provider has not reported usage yet.
    pub(in crate::tui) live_stream: super::usage_cost::LiveStreamUsageEstimate,
    /// Main-agent prompt-cache miss detector and session re-bill totals.
    pub(in crate::tui) cache_stats: super::cache_stats::CacheStatsTracker,
    // Cumulative cost from completed subagents (bg + fg), claimed once per run via
    // SubagentManager::claim_terminal_costs_usd_micros during panel refresh.
    pub(in crate::tui) subagent_total_cost_usd_micros: u64,
    // Cumulative cost from finished advisor calls, claimed via
    // AdvisorSessionStore::claim_cost_usd_micros during the same poll path.
    pub(in crate::tui) advisor_total_cost_usd_micros: u64,
}

impl UsageUi {
    /// Non-main session cost folded into the statusline total.
    pub(in crate::tui) fn extra_cost_usd_micros(&self) -> u64 {
        self.subagent_total_cost_usd_micros
            .saturating_add(self.advisor_total_cost_usd_micros)
    }
}

/// Who put the current status text up. Lets background polls retire their own
/// message without comparing the status against a copy string that a rewording
/// would silently break.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum StatusSource {
    #[default]
    Other,
    McpConnecting,
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
    TextInput(super::text_input::TextInput),
    InteractivePending(LoginTarget),
    InlineChoice(InlineChoiceModal),
    Questionnaire(QuestionnaireComposer),
    Approval(ApprovalComposer),
    Limits(limits_command::LimitsOverlay),
    Side,
}

impl ComposerMode {
    /// Whether a turn held during MCP connect must keep waiting. Every mode
    /// except plain input owns the keyboard, and during-turn key routing has no
    /// arm for them, so releasing underneath one would leave it painted but
    /// deaf to its own keys.
    pub(super) fn blocks_held_turn_start(&self) -> bool {
        match self {
            Self::Input => false,
            Self::Picker(_)
            | Self::SecretInput(_)
            | Self::ConfigNumberInput(_)
            | Self::TextInput(_)
            | Self::InteractivePending(_)
            | Self::InlineChoice(_)
            | Self::Questionnaire(_)
            | Self::Approval(_)
            | Self::Limits(_)
            | Self::Side => true,
        }
    }

    pub(super) fn blocks_auto_continue(&self) -> bool {
        match self {
            Self::InlineChoice(modal) => modal.blocks_auto_continue(),
            _ => false,
        }
    }

    pub(super) fn is_centered_overlay(&self) -> bool {
        match self {
            Self::Picker(picker) => picker.is_overlay(),
            Self::Limits(_) | Self::Side => true,
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

#[derive(Clone, Debug, PartialEq)]
pub(super) struct QueuedPrompt {
    pub(in crate::tui) prompt: String,
    pub(in crate::tui) display_prompt: String,
    pub(in crate::tui) paste_segments: Vec<PasteSegment>,
    pub(in crate::tui) media: Vec<ChatMedia>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InputDraft {
    pub(in crate::tui) input: String,
    pub(in crate::tui) paste_segments: Vec<PasteSegment>,
    pub(in crate::tui) submission_mode: InputSubmissionMode,
    pub(in crate::tui) shell_mode: Option<InlineShellMode>,
}

impl From<&str> for QueuedPrompt {
    fn from(prompt: &str) -> Self {
        Self {
            prompt: prompt.to_string(),
            display_prompt: prompt.to_string(),
            paste_segments: Vec::new(),
            media: Vec::new(),
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
    Failed(Box<FailedTurn>),
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
    /// A prompt offered by a connected MCP server. Expanded on submit, because
    /// `prompts/get` is a round-trip the palette cannot make.
    McpPrompt,
    /// One value a server suggested for the prompt argument under the cursor.
    /// Carries the char range it fills so picking it settles that argument
    /// alone, leaving the command and any other arguments as typed.
    McpPromptArgument {
        value: std::ops::Range<usize>,
    },
}

/// Clock for the live elapsed decoration: preserved across card replacement,
/// started when a card is first seen running, absent otherwise.
pub(super) fn live_started_at(
    previous: Option<&ToolEntry>,
    status: rho_tools::tool_card::ToolStatus,
) -> Option<Instant> {
    previous
        .and_then(|entry| entry.started_at)
        .or_else(|| matches!(status, rho_tools::tool_card::ToolStatus::Running).then(Instant::now))
}

/// Keyed paint of one live tool card.
///
/// Lives on [`ToolEntry`] so card replacement drops it. Hits skip syntect.
/// Elapsed ticks rebuild the cheap prefix and reuse `body`.
pub(in crate::tui) struct LiveCardRenderCache {
    pub(in crate::tui) width: usize,
    pub(in crate::tui) max_tool_output_lines: usize,
    pub(in crate::tui) max_image_height: u16,
    pub(in crate::tui) theme_generation: u64,
    pub(in crate::tui) expanded: bool,
    pub(in crate::tui) elapsed_label: Option<String>,
    pub(in crate::tui) last_fact_is_end: bool,
    pub(in crate::tui) prefix_len: usize,
    pub(in crate::tui) body: Vec<Line<'static>>,
    pub(in crate::tui) lines: Vec<Line<'static>>,
    #[cfg(test)]
    pub(in crate::tui) paints: u32,
}

impl std::fmt::Debug for LiveCardRenderCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveCardRenderCache")
            .field("width", &self.width)
            .field("max_tool_output_lines", &self.max_tool_output_lines)
            .field("max_image_height", &self.max_image_height)
            .field("theme_generation", &self.theme_generation)
            .field("expanded", &self.expanded)
            .field("elapsed_label", &self.elapsed_label)
            .field("body_lines", &self.body.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) struct ToolEntry {
    /// Structured Call + Children card. Sole render input for tool rows.
    /// In-place body edits must clear [`Self::render_cache`]; replacement
    /// through [`Self::new`] drops it automatically.
    pub(in crate::tui) card: rho_tools::tool_card::ToolCard,
    pub(in crate::tui) expanded: bool,
    pub(in crate::tui) image: Option<FeedImage>,
    /// Wall clock for live shell elapsed (`timeout … · 1.2s`) while a shell
    /// call runs. Set when a tool starts running; preserved across card
    /// updates; absent on historical, finished, interrupted, and preview rows.
    pub(in crate::tui) started_at: Option<Instant>,
    /// Live-layout paint cache. Boxed so [`Entry::Tool`] stays small; not cloned.
    pub(in crate::tui) render_cache: Option<Box<LiveCardRenderCache>>,
}

impl Clone for ToolEntry {
    fn clone(&self) -> Self {
        Self {
            card: self.card.clone(),
            expanded: self.expanded,
            image: self.image.clone(),
            started_at: self.started_at,
            render_cache: None,
        }
    }
}

impl ToolEntry {
    pub(in crate::tui) fn new(
        card: rho_tools::tool_card::ToolCard,
        expanded: bool,
        image: Option<FeedImage>,
        started_at: Option<Instant>,
    ) -> Self {
        Self {
            card,
            expanded,
            image,
            started_at,
            render_cache: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum Entry {
    User(String),
    Assistant(AssistantEntry),
    Reasoning(ReasoningEntry),
    Tool(ToolEntry),
    Notice(String),
    RuntimeInfo(Box<info_command::RuntimeInfo>),
    Changelog(Box<crate::changelog::ChangelogDisplay>),
    Error(String),
}

/// Streamed assistant text plus optional post-turn duration receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AssistantEntry {
    pub(in crate::tui) text: String,
    pub(in crate::tui) worked_for: Option<Duration>,
}

impl AssistantEntry {
    pub(super) fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            worked_for: None,
        }
    }

    pub(super) fn summary_only(worked_for: Duration) -> Self {
        Self {
            text: String::new(),
            worked_for: Some(worked_for),
        }
    }

    pub(super) fn push_str(&mut self, text: &str) {
        self.text.push_str(text);
    }
}

impl From<&str> for AssistantEntry {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for AssistantEntry {
    fn from(text: String) -> Self {
        Self::new(text)
    }
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
            Self::Assistant => Entry::Assistant(text.into()),
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

#[derive(Clone, Copy, Debug)]
pub(super) enum HistoryDirection {
    Previous,
    Next,
}
