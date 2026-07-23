use std::{
    collections::VecDeque,
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use history_cache::{CachedCodeBlock, HistoryLineCache};
use questionnaire::QuestionnaireCancelReason;
#[cfg(test)]
use std::sync::Mutex;
use tokio::sync::oneshot;
use tool_call_batch::ToolCallBatch;

#[cfg(test)]
use ratatui::layout::Rect;
use ratatui::{
    style::{Modifier, Style},
    text::Line,
    DefaultTerminal,
};
mod activity;
mod agent_picker;
mod app_construct;
mod approval;
mod attachment;
mod background_polls;
mod clipboard;
mod command_actions;
mod command_block;
mod command_palette;
mod composer;
mod config_actions;
mod config_editor;
mod config_input;
mod config_picker;
mod context_handoff;
mod copy_interaction;
mod doctor;
mod event_adapter;
mod feed_image;
mod file_palette;
mod file_picker;
mod frame_scheduler;
mod goal;
pub(crate) use goal::GOAL_JUDGE_PROMPT;
mod choice_actions;
mod during_turn;
mod goal_command;
mod help_picker;
mod history_cache;
mod info_command;
mod inline_choice;
mod inline_shell;
mod keybindings;
mod keyboard_modes;
mod limits_command;
mod local_commands;
mod local_diff;
mod login;
mod markdown;
mod markdown_image;
mod message_history;
mod message_render;
mod model_actions;
mod model_picker;
mod mouse;
mod mouse_capture;
mod paste_burst;
mod pending_input;
mod permission_mode;
mod picker;
mod picker_input;
mod picker_overlay;
mod prompt_turn;
mod provider_attempt;
mod provider_picker;
mod questionnaire;
mod questionnaire_input;
mod reasoning_metadata;
mod render;
mod rendered_entry;
mod run_lifecycle;
mod screen_layout;
mod scrollbar;
mod session_actions;
mod session_picker;
mod session_title;
mod transcript_events;
pub(crate) use session_title::SESSION_TITLE_PROMPT;
mod app_loop;
mod helpers;
mod idle_input;
mod reasoning_phase;
mod skill_actions;
mod skill_picker;
#[cfg(debug_assertions)]
mod smoke_injection;
mod statusline;
mod stream;
mod stream_preview;
mod subagent_panel;
mod terminal_events;
mod text_selection;
mod theme;
mod tool_call_batch;
mod tool_diff;
mod tool_output_ui;
mod tree_actions;
mod turn_prompt;
mod usage_cost;
mod view;
mod workspace;

use activity::{ActivityPhase, ActivityStatus, LoadingSpinner};
use approval::{approval_lines, ApprovalComposer, ApprovalKeyOutcome};
use clipboard::ClipboardWriter;
use config_editor::{
    config_number_input_lines, config_text_input_lines, resolve_web_search_editor_value,
    ConfigMutation, ConfigNumberInput, ConfigNumberKey, ConfigTextInput, ConfigTextKey,
    ConfigToggle,
};
use copy_interaction::CodeBlockCopyTarget;
use event_adapter::{SdkEventAdapter, ViewEvent, ViewModelEvent};
use feed_image::FeedImage;
use frame_scheduler::FrameScheduler;
use goal::GoalState;
use inline_choice::{
    InlineChoice, InlineChoiceKeyOutcome, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending,
};
use inline_shell::InlineShellMode;
use login::{PendingOAuthLogin, SecretInput};
use markdown::CodeFenceState;
use paste_burst::{PasteBurst, PasteBurstEnter};
use pending_input::{AcceptedSteering, PendingInputAction, PendingInputPanel};
use picker::{
    sort_items_by_ascii_label, PickerAction, PickerBadge, PickerBadgePlacement, PickerBadgeTone,
    PickerItem, PickerLayout, UiPicker,
};
use prompt_turn::FailedTurn;
use provider_attempt::ProviderAttempt;
use questionnaire::{
    questionnaire_cursor_position, questionnaire_lines, questionnaire_notice_text,
    QuestionAnswerRequest, QuestionnaireComposer, QuestionnaireReply, QuestionnaireResponseChannel,
};
use render::{
    char_prefix_display_width, display_width, entry_lines, input_cursor_index_on_visual_line,
    input_cursor_position, input_lines_with_images, input_visual_lines, labeled_divider_line,
    picker_lines, session_header_lines, styled_line, tool_entry_lines, truncate_one_line, LineFill,
};
use scrollbar::{scroll_state_for_top_line, HistoryScrollbar, HistoryScrollbarDrag};
use session_title::PendingSessionTitle;
use statusline::{GoalStatus, StatusLine};
use stream::AppendOnlyStream;
use subagent_panel::SubagentPanel;
use terminal_events::TerminalEvents;
use text_selection::{highlight_selection, render_copy_notice, CopyNotice, TextSelection};
use theme::Theme;
use turn_prompt::TurnPrompt;
use usage_cost::{estimated_cost_usd_micros, UsageCostTracker};

use helpers::{
    add_optional, complete_slash_command, expand_paste_segments, expandable_tool_entry,
    final_answer_delta, is_tool_entry, merge_usage, next_word_boundary, normalize_paste,
    oauth_pending_lines, pad_display_line, padded_content_width, paste_marker_for,
    previous_word_boundary, print_exit_summary, questionnaire_reply, recovered_history_tail,
    render_message_blocks, render_user_entry, secret_input_lines, short_session_id,
    slash_command_args, text_blocks, tool_display_line_count, usage_difference,
    usage_with_estimated_cost, visible_composer_start,
};
use message_history::transcript_entries_from_messages;

use {
    crate::app::config_repository::ConfigRepository,
    crate::app::interactive_runtime::InteractiveRuntime,
    crate::commands::{self, CommandId, CommandInvocation, CommandSpec},
    crate::herdr::{HerdrReporter, HerdrState},
    crate::keybindings::Keybindings,
    crate::permission::PermissionMode,
    crate::session::Session,
    rho_providers::credentials::CredentialStore,
    rho_providers::model::{
        catalog::{self, LoginTarget, ModelSelection},
        favorites,
        provider_models::refresh_provider_models_with_store,
        ContentBlock, ContextUsage, ImageContent, Message, ModelMetadata, ModelUsage,
        ReasoningRequestSource, UnavailableProvider,
    },
    rho_providers::provider,
    rho_providers::reasoning::ReasoningLevel,
    rho_tools::tool::ToolDisplayStyle,
};
const DEFAULT_TUI_HEIGHT: u16 = 18;
const PASTE_COLLAPSE_MIN_LINES: usize = 2;
const PASTE_COLLAPSE_MIN_CHARS: usize = 1000;
const MAX_COMMAND_SUGGESTIONS: usize = 5;
const MIN_COMMAND_DESCRIPTION_WIDTH: usize = 7;
const MAX_TERMINAL_EVENTS_PER_TICK: usize = 256;
const RECOVERED_HISTORY_LINE_LIMIT: usize = 200;
const STREAM_PREVIEW_DELAY: Duration = Duration::from_millis(24);
const STREAM_PREVIEW_MIN_CHARS: usize = 2;
const HISTORY_SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(1200);
pub struct TuiBootstrap {
    pub runtime: RuntimeModelView,
    pub session: SessionBootstrap,
    pub services: ApplicationServices,
}

pub struct RuntimeModelView {
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
    pub(crate) model_aliases: crate::model_aliases::ModelAliases,
    pub reasoning: ReasoningLevel,
    pub reasoning_source: ReasoningRequestSource,
    pub permission_mode: PermissionMode,
    pub show_reasoning_output: bool,
    pub auth: String,
    pub internal_agents:
        std::collections::BTreeMap<String, crate::config::InternalAgentModelConfig>,
    pub favorite_models: Vec<String>,
    pub max_tool_output_lines: usize,
    pub keybindings: Keybindings,
    pub prompt_templates: crate::prompt_templates::PromptTemplates,
}

pub struct SessionBootstrap {
    pub session_id: Option<String>,
    pub recovered_messages: Vec<Message>,
    pub open_resume_picker: bool,
}

pub struct ApplicationServices {
    pub(crate) config_repository: ConfigRepository,
    pub auth_unavailable: Option<String>,
    pub update_notice: Option<String>,
    pub pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pub diagnostics: crate::diagnostics::RuntimeDiagnostics,
    pub herdr: HerdrReporter,
}
pub struct TuiResult {
    pub resume_session_id: Option<String>,
    exit_summary: Option<String>,
}
pub(crate) use attachment::{run as run_attachment, AttachmentWriter};

pub async fn run(agent: &mut InteractiveRuntime, info: TuiBootstrap) -> anyhow::Result<TuiResult> {
    let mut terminal = ratatui::init();
    Theme::initialize_from_terminal();
    let keyboard = keyboard_modes::Enabled::acquire();
    let mouse_capture_enabled = mouse_capture::enable().is_ok();
    let herdr = info.services.herdr.clone();
    let herdr_graphics = herdr.graphics_capability().await;
    let initial_state = if info.services.auth_unavailable.is_some() {
        HerdrState::Blocked
    } else {
        HerdrState::Idle
    };
    herdr
        .report_state(
            initial_state,
            info.services.auth_unavailable.as_deref(),
            info.session.session_id.as_deref(),
        )
        .await;
    let result = {
        #[cfg(debug_assertions)]
        let injected = smoke_injection::after_terminal_init();
        #[cfg(not(debug_assertions))]
        let injected: anyhow::Result<()> = Ok(());

        match injected {
            Ok(()) => {
                App::new(info, herdr_graphics)
                    .run(&mut terminal, agent)
                    .await
            }
            Err(error) => Err(error),
        }
    };
    herdr.release().await;
    keyboard.release();
    if mouse_capture_enabled {
        let _ = mouse_capture::disable();
    }
    ratatui::restore();
    if let Ok(result) = &result {
        print_exit_summary(result.exit_summary.as_deref())?;
    }
    result
}

#[cfg(test)]
struct ActiveFrame {
    lines: Vec<Line<'static>>,
}
struct LiveStreamPreview {
    kind: StreamKind,
    text: String,
    include_leading_blank: bool,
}
struct SessionHeaderCache {
    width: usize,
    update_notice: Option<String>,
    lines: Vec<Line<'static>>,
}

#[derive(Debug, PartialEq, Eq)]
struct InteractiveModelSelection {
    selection: ModelSelection,
    alias: Option<String>,
}

struct App {
    info: TuiBootstrap,
    terminal_events: Option<TerminalEvents>,
    statusline: StatusLine,
    subagent_panel: SubagentPanel,
    input: String,
    input_cursor: usize,
    /// Explicit shell-mode state. Composer text stores only the command body.
    shell_mode: Option<InlineShellMode>,
    status: String,
    should_quit: bool,
    ctrl_c_streak: u8,
    assistant_stream: AppendOnlyStream,
    assistant_stream_code_fence: CodeFenceState,
    reasoning_stream: AppendOnlyStream,
    reasoning_stream_code_fence: CodeFenceState,
    current_stream_kind: Option<StreamKind>,
    stream_preview_deadline: Option<Instant>,
    live_stream_preview: Option<LiveStreamPreview>,
    current_turn_start: Option<usize>,
    provider_attempt: ProviderAttempt,
    reasoning_phase: reasoning_phase::ReasoningPhase,
    session_ui: run_lifecycle::SessionUiPhase,
    activity_phase: ActivityPhase,
    loading_spinner: LoadingSpinner,
    tool_calls: ToolCallBatch,
    image_picker: Option<ratatui_image::picker::Picker>,
    steering_prompts: VecDeque<QueuedPrompt>,
    accepted_steering: VecDeque<AcceptedSteering>,
    retracting_steering: Option<rho_sdk::SteeringId>,
    pending_input_panel: PendingInputPanel,
    pending_input_action: Option<PendingInputAction>,
    queued_prompts: VecDeque<QueuedPrompt>,
    pending_inline_shells: Vec<inline_shell::PendingShellTask>,
    deferred_inline_shell_context: Vec<inline_shell::DeferredShellContext>,
    goal: Option<GoalState>,
    pending_images: Vec<ImageContent>,
    input_history: Vec<String>,
    input_history_cursor: Option<usize>,
    input_history_draft: Option<InputDraft>,
    paste_burst: PasteBurst,
    paste_segments: Vec<PasteSegment>,
    input_submission_mode: InputSubmissionMode,
    transcript: Vec<Entry>,
    history_lines: HistoryLineCache,
    last_status_notice: Option<String>,
    last_inserted_was_tool: bool,
    command_selection: usize,
    command_prefix: Option<String>,
    command_palette_dismissed: bool,
    file_selection: usize,
    file_query: Option<String>,
    file_palette_dismissed: bool,
    file_match_cache: Option<FileMatchCache>,
    skill_match_cache: Option<SkillMatchCache>,
    composer: ComposerMode,
    credential_store: Arc<dyn CredentialStore>,
    available_auths: Vec<String>,
    using_unavailable_provider: bool,
    pending_oauth_login: Option<PendingOAuthLogin>,
    pending_usage_limits: Option<tokio::task::JoinHandle<limits_command::LimitsFetchResult>>,
    usage_limits_client: reqwest::Client,
    cumulative_usage: Option<ModelUsage>,
    usage_cost_tracker: UsageCostTracker,
    // SDK usage updates are cumulative within a run. These snapshots let the TUI
    // replace active usage while preserving totals from prior runs and steps.
    usage_before_current_run: Option<ModelUsage>,
    usage_before_current_step: Option<ModelUsage>,
    usage_before_current_attempt: Option<ModelUsage>,
    current_run_usage: Option<ModelUsage>,
    latest_usage: Option<ModelUsage>,
    current_context: Option<ContextUsage>,
    model_metadata: Option<ModelMetadata>,
    pending_model_metadata: Option<tokio::task::JoinHandle<Option<ModelMetadata>>>,
    pending_model_metadata_reasoning: Option<(ReasoningLevel, ReasoningRequestSource)>,
    pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pending_model_selection: Option<InteractiveModelSelection>,
    internal_agent_model_target: Option<String>,
    pending_session_title: Option<PendingSessionTitle>,
    markdown_images: markdown_image::MarkdownImageCache,
    markdown_images_dirty_from: Option<usize>,
    history_scroll: HistoryScroll,
    history_scrollbar_drag: Option<HistoryScrollbarDrag>,
    history_scrollbar_visible_until: Option<Instant>,
    history_scrollbar_hovered: bool,
    hovered_code_block_copy: Option<usize>,
    text_selection: Option<TextSelection>,
    copy_notice: Option<CopyNotice>,
    clipboard: Box<dyn ClipboardWriter + Send>,
    session_header_cache: Option<SessionHeaderCache>,
    last_mouse_position: Option<(u16, u16)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum InputSubmissionMode {
    #[default]
    ParseCommands,
    Prompt,
}

#[derive(Debug)]
enum ComposerMode {
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
    fn blocks_auto_continue(&self) -> bool {
        match self {
            Self::InlineChoice(modal) => modal.blocks_auto_continue(),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PasteSegment {
    start: usize,
    marker_len: usize,
    content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueuedPrompt {
    prompt: String,
    display_prompt: String,
    paste_segments: Vec<PasteSegment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InputDraft {
    input: String,
    paste_segments: Vec<PasteSegment>,
    submission_mode: InputSubmissionMode,
    shell_mode: Option<InlineShellMode>,
}

#[derive(Clone, Debug)]
struct FileMatchCache {
    query: String,
    matches: std::sync::Arc<Vec<String>>,
    refreshed_at: Instant,
}

/// Discovered skills reused across command palette queries, so typing a slash
/// command does not re-walk skill directories on every keystroke.
struct SkillMatchCache {
    skills: std::sync::Arc<Vec<crate::skills::Skill>>,
    refreshed_at: Instant,
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
    fn end(&self) -> usize {
        self.start + self.marker_len
    }
}

#[derive(Debug)]
struct SessionTitleResult {
    session_id: String,
    title: anyhow::Result<String>,
}

#[derive(Clone, Debug)]
struct CommandChoice {
    name: String,
    usage: String,
    description: String,
    kind: CommandChoiceKind,
}

#[derive(Debug, PartialEq)]
enum TurnOutcome {
    Completed,
    Interrupted,
    /// User cancelled interactive work such as a questionnaire.
    Cancelled,
    Failed(FailedTurn),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TurnOutcomeKind {
    Completed,
    Interrupted,
    Cancelled,
    Failed,
}

impl TurnOutcome {
    fn kind(&self) -> TurnOutcomeKind {
        match self {
            Self::Completed => TurnOutcomeKind::Completed,
            Self::Interrupted => TurnOutcomeKind::Interrupted,
            Self::Cancelled => TurnOutcomeKind::Cancelled,
            Self::Failed(_) => TurnOutcomeKind::Failed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryScroll {
    Bottom,
    Manual { top_line: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CommandChoiceKind {
    Builtin(&'static CommandSpec),
    BuiltinArgument(&'static commands::CommandArgumentChoice),
    PromptTemplate(String),
    Skill,
}

#[derive(Clone, Debug)]
struct ToolEntry {
    state: ToolEntryState,
    display_lines: Vec<String>,
    expanded: bool,
    image: Option<FeedImage>,
}

#[derive(Clone, Copy, Debug)]
enum ToolEntryState {
    Running,
    Finished {
        ok: bool,
        display_style: ToolDisplayStyle,
    },
}

#[derive(Clone, Debug)]
enum Entry {
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
struct ReasoningEntry {
    text: String,
    thought_for: Option<Duration>,
}

impl ReasoningEntry {
    fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            thought_for: None,
        }
    }

    fn summary_only(thought_for: Duration) -> Self {
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
    fn is_provider_replaceable(&self) -> bool {
        matches!(self, Self::Assistant(_) | Self::Reasoning(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamKind {
    Assistant,
    Reasoning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteBurstKey {
    Char(char),
    Enter,
}

#[derive(Debug, PartialEq, Eq)]
enum FinalAnswerDelta<'a> {
    None,
    Append(&'a str),
    Mismatch,
}

impl StreamKind {
    fn style(self) -> Style {
        match self {
            Self::Assistant => Theme::text(),
            Self::Reasoning => Theme::dim().add_modifier(Modifier::DIM),
        }
    }

    fn entry(self, text: String) -> Entry {
        match self {
            Self::Assistant => Entry::Assistant(text),
            Self::Reasoning => Entry::Reasoning(ReasoningEntry::new(text)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamControl {
    Continue,
    Interrupt,
    Resize,
    ApprovalResolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HerdrUserWait {
    Approval,
    Questionnaire,
}

impl HerdrUserWait {
    const fn message(self) -> &'static str {
        match self {
            Self::Approval => "waiting for approval",
            Self::Questionnaire => "waiting for your answers",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunningInputMode {
    Turn,
    Compacting,
}

#[derive(Clone, Copy, Debug)]
enum HistoryDirection {
    Previous,
    Next,
}

#[cfg(test)]
#[path = "tui/app_tests.rs"]
mod tests;
