use std::{
    collections::VecDeque,
    future::Future,
    io::Write,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use futures_util::{task::noop_waker_ref, FutureExt};
use history_cache::{CachedCodeBlock, HistoryLineCache};
use questionnaire::QuestionnaireCancelReason;
#[cfg(test)]
use std::sync::Mutex;
use tokio::sync::oneshot;
use tool_call_batch::ToolCallBatch;

use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
};
#[cfg(test)]
use ratatui::layout::Rect;
use ratatui::{
    backend::Backend,
    style::{Modifier, Style},
    text::{Line, Span},
    DefaultTerminal, Terminal,
};
mod activity;
mod agent_picker;
mod approval;
mod attachment;
mod clipboard;
mod command_actions;
mod command_block;
mod command_palette;
mod composer;
mod config_actions;
mod config_editor;
mod config_picker;
mod copy_interaction;
mod doctor;
mod event_adapter;
mod feed_image;
mod file_palette;
mod file_picker;
mod frame_scheduler;
mod goal;
pub(crate) use goal::GOAL_JUDGE_PROMPT;
mod goal_command;
mod history_cache;
mod info_command;
mod inline_shell;
mod keybindings;
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
pub(crate) use session_title::SESSION_TITLE_PROMPT;
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
mod tree_actions;
mod turn_prompt;
mod usage_cost;
mod view;
mod workspace;

use crate::clipboard::read_clipboard_image;
use activity::{ActivityPhase, ActivityStatus, LoadingSpinner};
use approval::{approval_lines, ApprovalComposer};
use clipboard::{ClipboardWriter, SystemClipboard};
use config_editor::{
    config_number_input_lines, config_text_input_lines, resolve_web_search_editor_value,
    ConfigMutation, ConfigNumberInput, ConfigNumberKey, ConfigNumberSave, ConfigTextInput,
    ConfigTextKey, ConfigToggle,
};
use copy_interaction::CodeBlockCopyTarget;
use event_adapter::{SdkEventAdapter, ViewEvent, ViewModelEvent};
use feed_image::{picker_from_environment, FeedImage};
use frame_scheduler::FrameScheduler;
use goal::GoalState;
use inline_shell::InlineShellMode;
use login::{PendingOAuthLogin, SecretInput};
use markdown::{update_code_block_state, CodeFenceState};
use paste_burst::{PasteBurst, PasteBurstEnter};
use pending_input::{AcceptedSteering, PendingInputAction, PendingInputPanel};
use picker::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, UiPicker};
use prompt_turn::FailedTurn;
use provider_attempt::ProviderAttempt;
use questionnaire::{
    questionnaire_cursor_position, questionnaire_lines, questionnaire_notice_text,
    QuestionAnswerRequest, QuestionnaireComposer, QuestionnaireReply, QuestionnaireResponseChannel,
};
use render::{
    char_prefix_display_width, display_width, entry_lines, input_cursor_index_on_visual_line,
    input_cursor_position, input_lines_with_images, input_visual_lines, picker_lines,
    session_header_lines, styled_line, tool_entry_lines, truncate_one_line, LineFill,
};
use rho_providers::model::ReasoningRequestSource::PersistedOrDefault;
use scrollbar::{scroll_state_for_top_line, HistoryScrollbar, HistoryScrollbarDrag};
use session_title::{generate_session_title, PendingSessionTitle};
use statusline::{GoalStatus, StatusLine};
use stream::{AppendOnlyStream, StreamFragment};
use subagent_panel::SubagentPanel;
use terminal_events::TerminalEvents;
use text_selection::{highlight_selection, render_copy_notice, CopyNotice, TextSelection};
use theme::Theme;
use turn_prompt::TurnPrompt;
use usage_cost::{estimated_cost_usd_micros, CostSource, UsageCostTracker};

use {
    crate::app::config_repository::ConfigRepository,
    crate::app::interactive_runtime::InteractiveRuntime,
    crate::commands::{self, CommandId, CommandInvocation, CommandSpec},
    crate::credential_store::{build_provider as build_sdk_provider, AppCredentialStore},
    crate::herdr::{HerdrReporter, HerdrState},
    crate::keybindings::Keybindings,
    crate::permission::PermissionMode,
    crate::session::Session,
    rho_providers::credentials::{available_auth_modes, CredentialStore},
    rho_providers::model::{
        catalog::{self, LoginTarget, ModelSelection},
        favorites, image_summary,
        models_dev::fetch_model_metadata,
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
    let bracketed_paste_enabled = enable_bracketed_paste().is_ok();
    let mouse_capture_enabled = mouse_capture::enable().is_ok();
    let modified_keys_enabled = enable_modified_keys().is_ok();
    let keyboard_enhancements_enabled = enable_keyboard_enhancements().is_ok();
    let herdr = info.services.herdr.clone();
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
            Ok(()) => App::new(info).run(&mut terminal, agent).await,
            Err(error) => Err(error),
        }
    };
    herdr.release().await;
    if keyboard_enhancements_enabled {
        let _ = disable_keyboard_enhancements();
    }
    if modified_keys_enabled {
        let _ = disable_modified_keys();
    }
    if mouse_capture_enabled {
        let _ = mouse_capture::disable();
    }
    if bracketed_paste_enabled {
        let _ = disable_bracketed_paste();
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
    active_turn_show_reasoning_output: bool,
    hidden_reasoning_active: bool,
    running: bool,
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
    Questionnaire(QuestionnaireComposer),
    Approval(ApprovalComposer),
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
    Reasoning(String),
    Tool(ToolEntry),
    Notice(String),
    RuntimeInfo(Box<info_command::RuntimeInfo>),
    UsageLimits(crate::usage_limits::ProviderLimits),
    Error(String),
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

async fn questionnaire_reply(
    pending: &mut Option<(
        rho_sdk::ToolCallId,
        rho_sdk::HostInputId,
        oneshot::Receiver<QuestionnaireReply>,
    )>,
) -> Option<(
    rho_sdk::ToolCallId,
    rho_sdk::HostInputId,
    QuestionnaireReply,
)> {
    let (call_id, request_id, receiver) = pending.as_mut()?;
    let call_id = call_id.clone();
    let request_id = request_id.clone();
    let reply = receiver.await.ok();
    pending.take();
    reply.map(|reply| (call_id, request_id, reply))
}

fn is_tool_entry(entry: &Entry) -> bool {
    matches!(entry, Entry::Tool(_))
}

fn expandable_tool_entry(entry: &Entry, max_tool_output_lines: usize) -> bool {
    matches!(entry, Entry::Tool(tool) if tool_display_line_count(&tool.display_lines) > max_tool_output_lines)
}

#[derive(Debug, PartialEq, Eq)]
enum FinalAnswerDelta<'a> {
    None,
    Append(&'a str),
    Mismatch,
}

fn final_answer_delta<'a>(emitted_text: &str, answer: &'a str) -> FinalAnswerDelta<'a> {
    match answer.strip_prefix(emitted_text) {
        Some("") => FinalAnswerDelta::None,
        Some(suffix) => FinalAnswerDelta::Append(suffix),
        None => FinalAnswerDelta::Mismatch,
    }
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
            Self::Reasoning => Entry::Reasoning(text),
        }
    }
}

impl App {
    fn new(info: TuiBootstrap) -> Self {
        #[cfg(debug_assertions)]
        if smoke_injection::matrix_enabled() {
            return Self::new_with_credentials(
                info,
                Arc::new(rho_providers::credentials::MemoryCredentialStore::default()),
            );
        }
        Self::new_with_credentials(info, Arc::new(AppCredentialStore))
    }

    fn new_with_credentials(
        info: TuiBootstrap,
        credential_store: Arc<dyn CredentialStore>,
    ) -> Self {
        let available_auths = available_auth_modes(credential_store.as_ref());
        let using_unavailable_provider = info.services.auth_unavailable.is_some();
        let mut info = info;
        info.runtime.max_tool_output_lines = info.runtime.max_tool_output_lines.max(1);
        let status = info
            .services
            .auth_unavailable
            .as_ref()
            .map(|_| "no providers configured; run /login to sign in".into())
            .unwrap_or_else(|| "ready".into());
        let active_turn_show_reasoning_output = info.runtime.show_reasoning_output;
        let pending_update_notice = info.services.pending_update_notice.take();
        let statusline = StatusLine::new(&info.runtime);
        Self {
            info,
            terminal_events: None,
            statusline,
            subagent_panel: SubagentPanel::default(),
            input: String::new(),
            input_cursor: 0,
            status,
            should_quit: false,
            ctrl_c_streak: 0,
            assistant_stream: AppendOnlyStream::default(),
            assistant_stream_code_fence: CodeFenceState::default(),
            reasoning_stream: AppendOnlyStream::default(),
            reasoning_stream_code_fence: CodeFenceState::default(),
            current_stream_kind: None,
            stream_preview_deadline: None,
            live_stream_preview: None,
            current_turn_start: None,
            provider_attempt: ProviderAttempt::default(),
            active_turn_show_reasoning_output,
            hidden_reasoning_active: false,
            running: false,
            activity_phase: ActivityPhase::default(),
            loading_spinner: LoadingSpinner::default(),
            tool_calls: ToolCallBatch::default(),
            image_picker: picker_from_environment(),
            steering_prompts: VecDeque::new(),
            accepted_steering: VecDeque::new(),
            retracting_steering: None,
            pending_input_panel: PendingInputPanel::default(),
            pending_input_action: None,
            queued_prompts: VecDeque::new(),
            pending_inline_shells: Vec::new(),
            deferred_inline_shell_context: Vec::new(),
            goal: None,
            pending_images: Vec::new(),
            input_history: Vec::new(),
            input_history_cursor: None,
            input_history_draft: None,
            paste_burst: PasteBurst::default(),
            paste_segments: Vec::new(),
            input_submission_mode: InputSubmissionMode::default(),
            transcript: Vec::new(),
            history_lines: HistoryLineCache::default(),
            last_status_notice: None,
            last_inserted_was_tool: false,
            command_selection: 0,
            command_prefix: None,
            command_palette_dismissed: false,
            file_selection: 0,
            file_query: None,
            file_palette_dismissed: false,
            file_match_cache: None,
            skill_match_cache: None,
            composer: ComposerMode::Input,
            credential_store,
            available_auths,
            using_unavailable_provider,
            pending_oauth_login: None,
            pending_usage_limits: None,
            usage_limits_client: reqwest::Client::new(),
            cumulative_usage: None,
            usage_cost_tracker: UsageCostTracker::default(),
            usage_before_current_run: None,
            usage_before_current_step: None,
            usage_before_current_attempt: None,
            current_run_usage: None,
            latest_usage: None,
            current_context: None,
            model_metadata: None,
            pending_model_metadata: None,
            pending_model_metadata_reasoning: None,
            pending_update_notice,
            pending_model_selection: None,
            internal_agent_model_target: None,
            pending_session_title: None,
            markdown_images: markdown_image::MarkdownImageCache::default(),
            markdown_images_dirty_from: None,
            history_scroll: HistoryScroll::Bottom,
            history_scrollbar_drag: None,
            history_scrollbar_visible_until: None,
            history_scrollbar_hovered: false,
            hovered_code_block_copy: None,
            text_selection: None,
            copy_notice: None,
            clipboard: Box::new(SystemClipboard::default()),
            session_header_cache: None,
            last_mouse_position: None,
        }
    }

    async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TuiResult> {
        self.terminal_events = Some(TerminalEvents::new());
        self.start_model_metadata_fetch(agent);
        self.insert_session_intro(terminal)?;
        self.insert_recovered_history(terminal)?;
        if self.info.session.open_resume_picker {
            self.open_resume_picker()?;
        }
        if self.info.services.auth_unavailable.is_some() {
            self.insert_entry(&Entry::Notice(
                "no providers configured. run /login to sign in.".into(),
            ));
        }
        let mut needs_redraw = true;
        while !self.should_quit {
            let background_ready = self
                .pending_model_metadata
                .as_ref()
                .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_update_notice
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_oauth_login
                    .as_ref()
                    .is_some_and(|pending| pending.handle.is_finished())
                || self
                    .pending_usage_limits
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished());
            self.poll_model_metadata_fetch(agent);
            self.poll_update_notice();
            needs_redraw |= self.poll_pending_session_title()?;
            self.poll_pending_oauth_login(terminal, agent).await?;
            needs_redraw |= self.poll_limits_command().await?;
            needs_redraw |= self.poll_markdown_images();
            let shell_changed = self.finish_completed_inline_shells().await?;
            if !self.running {
                self.insert_deferred_inline_shell_context(agent)?;
            }
            needs_redraw |= shell_changed;
            needs_redraw |= background_ready;
            needs_redraw |= self.update_subagent_panel(agent);
            let terminal_input_ready = self.drain_ready_terminal_events(terminal, agent).await?;
            if self.should_quit {
                break;
            }
            if terminal_input_ready {
                needs_redraw = true;
                needs_redraw |= self.flush_due_paste_burst();
            } else {
                needs_redraw |= self.poll_subagent_completions(terminal, agent).await?;
            }
            if needs_redraw {
                terminal.draw(|frame| self.draw(frame))?;
                needs_redraw = false;
            }
            let subagents_active = agent.subagents().is_some_and(|manager| {
                manager.has_active_or_pending_notification(agent.session_id().as_str())
            });
            let idle_timeout = if self.pending_model_metadata.is_some()
                || self.pending_update_notice.is_some()
                || self.pending_session_title.is_some()
                || self.pending_oauth_login.is_some()
                || self.pending_usage_limits.is_some()
                || !self.pending_inline_shells.is_empty()
                || self.markdown_images.has_pending()
            {
                Duration::from_millis(100)
            } else if subagents_active {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(3600)
            };
            let redraw_on_timeout = self.animation_active(Instant::now());
            let timeout = self.event_poll_timeout(idle_timeout);
            tokio::select! {
                biased;
                event = self.terminal_events.as_mut().expect("terminal events initialized").next() => {
                    self.handle_terminal_event(event?, terminal, agent).await?;
                    needs_redraw = true;
                    self.drain_ready_terminal_events(terminal, agent).await?;
                    needs_redraw |= self.flush_due_paste_burst();
                }
                _ = tokio::time::sleep(timeout) => {
                    needs_redraw |= self.flush_due_paste_burst();
                    needs_redraw |= redraw_on_timeout;
                }
            }
        }
        self.cancel_limits_command().await;
        if let Some(mut pending) = self.pending_session_title.take() {
            pending.cancel();
            let _ = (&mut pending).await;
        }
        Ok(TuiResult {
            resume_session_id: self.info.session.session_id.clone(),
            exit_summary: self.exit_summary(),
        })
    }

    async fn drain_ready_terminal_events(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let mut handled = false;
        for _ in 1..MAX_TERMINAL_EVENTS_PER_TICK {
            let event = self
                .terminal_events
                .as_mut()
                .expect("terminal events initialized")
                .try_next();
            let Some(event) = event else {
                break;
            };
            self.handle_terminal_event(event?, terminal, agent).await?;
            handled = true;
            if self.should_quit {
                break;
            }
        }
        Ok(handled)
    }

    async fn handle_terminal_event(
        &mut self,
        event: Event,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                self.text_selection = None;
                self.handle_key(key, terminal, agent).await?;
            }
            Event::Paste(text) => {
                self.flush_pending_paste_burst();
                let text = normalize_paste(&text);
                self.insert_paste(&text);
                self.paste_burst.clear();
            }
            Event::Resize(_, _) => {
                self.flush_pending_paste_burst();
                self.text_selection = None;
                self.hovered_code_block_copy = None;
                self.hide_history_scrollbar();
                self.clamp_history_scroll_for_terminal(terminal)?;
            }
            Event::Mouse(mouse) => {
                self.handle_mouse_event(mouse.kind, mouse.column, mouse.row, terminal)?;
            }
            Event::FocusGained => {
                // Some Windows hosts drop application mouse tracking on focus
                // changes; re-assert so wheel scrolling keeps working.
                mouse_capture::reassert();
                self.statusline.refresh_git_branch();
            }
            Event::FocusLost | Event::Key(_) => {}
        }
        Ok(())
    }

    fn event_poll_timeout(&self, idle_timeout: Duration) -> Duration {
        let now = Instant::now();
        let timeout = self.paste_burst.poll_timeout(now, idle_timeout);
        let timeout = self
            .copy_notice
            .as_ref()
            .and_then(|notice| notice.visible_until().checked_duration_since(now))
            .map_or(timeout, |remaining| remaining.min(timeout));
        if self.history_scrollbar_hovered || self.history_scrollbar_drag.is_some() {
            return timeout;
        }
        self.history_scrollbar_visible_until
            .and_then(|visible_until| visible_until.checked_duration_since(now))
            .map_or(timeout, |remaining| remaining.min(timeout))
    }

    fn animation_active(&self, now: Instant) -> bool {
        self.loading_active()
            || self.subagent_panel.is_active()
            || self
                .copy_notice
                .as_ref()
                .is_some_and(|notice| now < notice.visible_until())
            || self.history_scrollbar_hovered
            || self.history_scrollbar_drag.is_some()
            || self
                .history_scrollbar_visible_until
                .is_some_and(|until| now < until)
    }

    async fn handle_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.handle_paste_burst_key(key) {
            return Ok(());
        }

        if self.handle_pending_input_key(key) {
            return Ok(());
        }

        if self.handle_history_key(key, terminal)? {
            return Ok(());
        }

        if self.handle_oauth_pending_key(key)? {
            return Ok(());
        }

        if self.handle_questionnaire_key(key)? {
            return Ok(());
        }

        if self.handle_secret_key(key, terminal, agent).await? {
            return Ok(());
        }

        if self.handle_config_number_key(key, terminal)? {
            return Ok(());
        }

        if self.handle_config_text_key(key)? {
            return Ok(());
        }

        if self.handle_reasoning_cycle_key(key, agent)? {
            return Ok(());
        }

        if self.handle_picker_key(key, terminal, agent).await? {
            return Ok(());
        }

        if self
            .handle_command_palette_key(key, terminal, agent)
            .await?
        {
            return Ok(());
        }

        if self.handle_file_palette_key(key)? {
            return Ok(());
        }

        if self.handle_configurable_composer_key(key, terminal, agent)? {
            return Ok(());
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    self.input.clear();
                    self.paste_segments.clear();
                    self.input_submission_mode = InputSubmissionMode::ParseCommands;
                    self.pending_images.clear();
                    self.input_cursor = 0;
                    self.clamp_command_selection();
                    self.notify_status("input cleared; press ctrl-c again to quit");
                    self.ctrl_c_streak = 1;
                } else {
                    self.should_quit = true;
                }
            }
            (_, KeyCode::Esc) => {
                self.cancel_inline_shells();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Backspace) => {
                self.delete_word_before_cursor();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Backspace) => {
                self.backspace_input();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Delete) => {
                self.delete_input();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Left) => {
                self.input_cursor = previous_word_boundary(&self.input, self.input_cursor);
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Right) => {
                self.input_cursor = next_word_boundary(&self.input, self.input_cursor);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Left) => {
                self.move_input_cursor_left();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Right) => {
                self.move_input_cursor_right();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Up) => {
                let width = terminal.size()?.width as usize;
                self.recall_input_history_or_move_cursor(HistoryDirection::Previous, width);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Down) => {
                let width = terminal.size()?.width as usize;
                self.recall_input_history_or_move_cursor(HistoryDirection::Next, width);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Home) => {
                self.reset_input_history_navigation();
                self.input_cursor = 0;
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::End) => {
                self.reset_input_history_navigation();
                self.input_cursor = self.input_char_len();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Enter) => {
                self.insert_input_char('\n');
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_input_char('\n');
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Enter) => {
                self.submit(terminal, agent).await?;
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_input_char(ch);
                self.ctrl_c_streak = 0;
            }
            _ => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
        }
        self.clamp_command_selection();
        self.clamp_file_selection();
        Ok(())
    }

    fn poll_update_notice(&mut self) {
        let Some(handle) = self.pending_update_notice.as_mut() else {
            return;
        };
        let Some(result) = handle.now_or_never() else {
            return;
        };
        self.pending_update_notice = None;
        if let Ok(Some(notice)) = result {
            self.info.services.update_notice = Some(notice);
        }
    }

    /// Wakes an idle session with a turn for finished background subagents.
    /// Real prompt turns drain these notifications themselves, while active
    /// goals deliver them before evaluating the goal again.
    async fn poll_subagent_completions(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !self.should_deliver_idle_subagent_completions() {
            return Ok(false);
        }
        Ok(self
            .run_subagent_completion_turn(terminal, agent)
            .await?
            .is_some())
    }

    async fn run_subagent_completion_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<Option<TurnOutcome>> {
        let Some(manager) = agent.subagents().cloned() else {
            return Ok(None);
        };
        let notifications = manager.take_notifications(agent.session_id().as_str());
        if notifications.is_empty() {
            return Ok(None);
        }
        // The whole drained batch is one message and one model request, no
        // matter how many runs finished while the parent was busy.
        let (model_prompt, display_prompt) =
            crate::tools::agent::notification_prompts(&notifications);
        self.run_prompt_turn(
            TurnPrompt::standard(model_prompt, display_prompt),
            Vec::new(),
            terminal,
            agent,
        )
        .await
        .map(Some)
    }

    fn should_deliver_idle_subagent_completions(&self) -> bool {
        !self.running && self.goal.is_none() && self.queued_prompts.is_empty()
    }

    fn start_model_metadata_fetch(&mut self, agent: &mut InteractiveRuntime) {
        if let Some(handle) = self.pending_model_metadata.take() {
            handle.abort();
        }
        self.pending_model_metadata_reasoning = None;
        if let Some((metadata, metadata_is_current)) = reasoning_metadata::cached_metadata(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        ) {
            agent.set_context_window(metadata.display_context_window());
            let reasoning_metadata_complete = metadata.reasoning_metadata_complete;
            self.model_metadata = Some(metadata);
            if reasoning_metadata_complete && metadata_is_current {
                return;
            }
        } else {
            agent.set_context_window(None);
            self.model_metadata = None;
        }
        let provider = self.info.runtime.provider.clone();
        let model = self.info.runtime.model.clone();
        self.pending_model_metadata_reasoning = Some((
            self.info.runtime.reasoning,
            self.info.runtime.reasoning_source,
        ));
        self.pending_model_metadata = Some(tokio::spawn(async move {
            fetch_model_metadata(&provider, &model).await
        }));
    }

    fn poll_model_metadata_fetch(&mut self, agent: &mut InteractiveRuntime) {
        let Some(handle) = self.pending_model_metadata.as_mut() else {
            return;
        };
        if !handle.is_finished() {
            return;
        }
        if let Some(handle) = self.pending_model_metadata.take() {
            let reasoning_at_fetch_start = self.pending_model_metadata_reasoning.take();
            if let Some(Ok(Some(metadata))) = handle.now_or_never() {
                agent.set_context_window(metadata.display_context_window());
                let capabilities = metadata.reasoning_capabilities();
                let resolved = reasoning_metadata::resolve_fetched_reasoning(
                    &capabilities,
                    self.info.runtime.reasoning,
                    reasoning_at_fetch_start,
                );
                let reasoning = resolved.effective;
                if let Some(requested) = resolved.rejected {
                    self.insert_entry(&Entry::Error(format!(
                        "reasoning level '{requested}' is not supported by {}/{}; restored '{reasoning}'",
                        self.info.runtime.provider, self.info.runtime.model
                    )));
                }
                let provider_updated = match build_sdk_provider(
                    &self.info.runtime.provider,
                    &self.info.runtime.model,
                    reasoning,
                ) {
                    Ok(provider) => match agent.replace_provider(provider, reasoning) {
                        Ok(_) => true,
                        Err(err) => {
                            self.insert_entry(&Entry::Error(format!(
                                "could not apply model reasoning metadata: {err}"
                            )));
                            false
                        }
                    },
                    Err(err) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not apply model reasoning metadata: {err}"
                        )));
                        false
                    }
                };
                if provider_updated && reasoning != self.info.runtime.reasoning {
                    self.info.set_reasoning(reasoning, PersistedOrDefault);
                    if let Err(err) = self.info.services.config_repository.update(|config| {
                        config.reasoning = reasoning;
                    }) {
                        self.insert_entry(&Entry::Error(format!(
                            "could not save normalized reasoning: {err}"
                        )));
                    }
                }
                self.model_metadata = Some(metadata);
            }
        }
    }

    fn poll_pending_session_title(&mut self) -> anyhow::Result<bool> {
        let Some(future) = self.pending_session_title.as_mut() else {
            return Ok(false);
        };
        let waker = noop_waker_ref();
        let mut context = std::task::Context::from_waker(waker);
        let std::task::Poll::Ready(result) = Pin::new(future).poll(&mut context) else {
            return Ok(false);
        };
        self.pending_session_title = None;
        let Ok(title) = result.title else {
            return Ok(false);
        };
        if Session::set_title(&self.info.runtime.cwd, &result.session_id, &title).is_err() {
            return Ok(false);
        }
        if self.info.session.session_id.as_deref() == Some(result.session_id.as_str()) {
            self.insert_entry(&Entry::Notice(format!("session titled: {title}")));
        }
        Ok(true)
    }

    fn handle_oauth_pending_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::OAuthPending(_)) {
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc => {
                let provider = if let Some(pending) = self.pending_oauth_login.take() {
                    let provider = pending.target.provider;
                    pending.handle.abort();
                    provider
                } else {
                    "OAuth".into()
                };
                self.composer = ComposerMode::Input;
                self.status = "login cancelled".into();
                self.insert_entry(&Entry::Notice(format!("{provider} login cancelled")));
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    async fn handle_secret_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let ComposerMode::SecretInput(secret) = &mut self.composer else {
            return Ok(false);
        };

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let target = secret.target.clone();
                let value = secret.value.trim().to_string();
                self.composer = ComposerMode::Input;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit_api_key_login(target, value, terminal, agent)
                    .await?;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.composer = ComposerMode::Input;
                self.status = "login cancelled".into();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Backspace) => {
                secret.backspace();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Delete) => {
                secret.delete();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Left) => {
                secret.cursor = secret.cursor.saturating_sub(1);
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Right) => {
                secret.cursor = (secret.cursor + 1).min(secret.char_len());
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Home) => {
                secret.cursor = 0;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::End) => {
                secret.cursor = secret.char_len();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                secret.insert_char(ch);
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn handle_config_number_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigNumberInput(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let ComposerMode::ConfigNumberInput(input) = &self.composer else {
                    return Ok(true);
                };
                let saved = match input.save(&self.info.services.config_repository) {
                    Ok(saved) => saved,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "config save failed".into();
                        return Ok(true);
                    }
                };
                match saved {
                    ConfigNumberSave::MaxOutputBytes(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::MAX_OUTPUT_BYTES_VALUE,
                        )?;
                        self.insert_entry(&Entry::Notice(format!(
                            "max output bytes set to {value}; applies next session"
                        )));
                    }
                    ConfigNumberSave::MaxToolOutputLines(value) => {
                        self.info.runtime.max_tool_output_lines = value;
                        self.info
                            .services
                            .diagnostics
                            .update_max_tool_output_lines(value);
                        self.open_main_config_picker_selected(
                            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE,
                        )?;
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        self.insert_entry(&Entry::Notice(format!(
                            "max tool output lines set to {value}"
                        )));
                    }
                    ConfigNumberSave::CompactThresholdPercent(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE,
                        )?;
                        self.insert_entry(&Entry::Notice(format!(
                            "compact threshold set to {value}%"
                        )));
                    }
                    ConfigNumberSave::CompactTargetPercent(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::COMPACT_TARGET_PERCENT_VALUE,
                        )?;
                        self.insert_entry(&Entry::Notice(format!(
                            "compact target set to {value}%"
                        )));
                    }
                }
                self.status = "config saved".into();
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.backspace();
                }
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.insert_char(ch);
                }
                Ok(true)
            }
            (_, KeyCode::Left) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.move_cursor_left();
                }
                Ok(true)
            }
            (_, KeyCode::Right) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.move_cursor_right();
                }
                Ok(true)
            }
            (_, KeyCode::Home) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.move_cursor_home();
                }
                Ok(true)
            }
            (_, KeyCode::End) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.move_cursor_end();
                }
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                let ComposerMode::ConfigNumberInput(input) = &self.composer else {
                    return Ok(true);
                };
                let selected_value = input.key.picker_value();
                let config = self.info.services.config_repository.load()?;
                self.info.runtime.show_reasoning_output = config.show_reasoning_output;
                self.open_main_config_picker_selected(selected_value)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn handle_config_text_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigTextInput(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let ComposerMode::ConfigTextInput(input) = &self.composer else {
                    return Ok(true);
                };
                let key = input.key;
                let save_result = input.save(self.credential_store.as_ref());
                match save_result {
                    Ok(()) => {
                        self.refresh_web_search_config_picker(key.picker_value())?;
                        self.status = format!("{} saved", key.label());
                    }
                    Err(err) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not save {}: {err}",
                            key.label()
                        )));
                        self.status = "config save failed".into();
                    }
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.backspace();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Delete) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.delete();
                }
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.insert_char(ch);
                }
                Ok(true)
            }
            (_, KeyCode::Left) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.move_cursor_left();
                }
                Ok(true)
            }
            (_, KeyCode::Right) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.move_cursor_right();
                }
                Ok(true)
            }
            (_, KeyCode::Home) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.move_cursor_home();
                }
                Ok(true)
            }
            (_, KeyCode::End) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.move_cursor_end();
                }
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                let ComposerMode::ConfigTextInput(input) = &self.composer else {
                    return Ok(true);
                };
                self.refresh_web_search_config_picker(input.key.picker_value())?;
                self.status = "web search config".into();
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn handle_reasoning_cycle_key(
        &mut self,
        key: KeyEvent,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let is_shift_tab = matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT));
        if !is_shift_tab {
            return Ok(false);
        }

        self.cycle_reasoning(agent)?;
        self.paste_burst.clear();
        self.ctrl_c_streak = 0;
        Ok(true)
    }

    async fn handle_picker_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::Picker(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.select_previous();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.select_next();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.complete_filter();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.pop_filter_char();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::CONTROL, KeyCode::Char('p')) if self.model_picker_is_open() => {
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Char(' ')) if self.picker_space_confirms_selection() => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit_picker_selection(terminal, agent).await?;
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.push_filter_char(ch);
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit_picker_selection(terminal, agent).await?;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.handle_picker_escape(/*running*/ false)?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    async fn handle_command_palette_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !self.command_palette_visible() {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.command_selection = if self.command_selection == 0 {
                        matches.len() - 1
                    } else {
                        self.command_selection - 1
                    };
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.command_selection = (self.command_selection + 1) % matches.len();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.command_palette_dismissed = false;
                    self.clamp_command_selection();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.clamp_command_selection();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit(terminal, agent).await?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.command_palette_dismissed = true;
                self.command_selection = 0;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn ensure_session(&mut self, agent: &mut InteractiveRuntime) -> anyhow::Result<()> {
        if self.info.session.session_id.is_none() {
            let session_id = agent.session_id().to_string();
            let (agent_id, agent_fingerprint) = agent.agent_identity();
            let session = Session::create_with_id(
                &self.info.runtime.cwd,
                &session_id,
                agent_id,
                agent_fingerprint,
            )?;
            self.info.session.session_id = Some(session_id);
            agent.attach_storage(session);
        }
        Ok(())
    }

    fn internal_agent_model_selection(&self, id: &str) -> (String, String, String) {
        self.info
            .runtime
            .internal_agents
            .get(id)
            .map(|selection| {
                (
                    selection.provider.clone(),
                    selection.model.clone(),
                    selection.auth.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    self.info.runtime.provider.clone(),
                    self.info.runtime.model.clone(),
                    self.info.runtime.auth.clone(),
                )
            })
    }

    fn start_session_title_generation(
        &mut self,
        first_user_message: String,
        agent: &InteractiveRuntime,
    ) {
        if self.info.session.session_id.is_none() {
            return;
        }
        let session_id = agent.session_id().clone();
        let workspace_path = agent.workspace_path().to_path_buf();
        let usage_recording = agent.usage_recording();
        self.pending_session_title = None;
        let (provider, model, _auth) =
            self.internal_agent_model_selection(crate::agent::SESSION_TITLE_AGENT_ID);
        let cancellation = rho_sdk::CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task_session_id = session_id.clone();
        let handle = tokio::spawn(async move {
            let title = generate_session_title(
                provider,
                model,
                first_user_message,
                task_session_id.clone(),
                workspace_path,
                usage_recording,
                task_cancellation,
            )
            .await;
            SessionTitleResult {
                session_id: task_session_id.to_string(),
                title,
            }
        });
        self.pending_session_title = Some(PendingSessionTitle::new(
            session_id.to_string(),
            cancellation,
            handle,
        ));
    }

    async fn submit(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let mut turn = TurnPrompt::standard(
            self.expanded_input().trim().to_string(),
            self.input.trim().to_string(),
        );
        if turn.model.is_empty() && self.pending_images.is_empty() {
            self.clear_submitted_input();
            return Ok(());
        }
        if let Some((mode, command)) = InlineShellMode::parse(self.input.trim()) {
            if !self.paste_segments.is_empty() {
                return self.block_pasted_inline_shell();
            }
            let command = command.to_string();
            self.clear_submitted_input();
            self.ensure_session(agent)?;
            self.start_inline_shell(mode, command)?;
            return Ok(());
        }

        match self.parse_input_command() {
            Ok(Some(mut invocation)) => {
                if invocation.id == CommandId::Goal {
                    invocation.raw_args = slash_command_args(&turn.model).to_string();
                    invocation.args = invocation.raw_args.trim().to_string();
                }
                self.input.clear();
                self.paste_segments.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                self.execute_command(invocation, terminal, agent).await?;
                return Ok(());
            }
            Ok(None) => {}
            Err(commands::CommandParseError::Unknown(name)) => {
                let trailing_prompt = slash_command_args(&turn.model).trim().to_string();
                self.input.clear();
                self.paste_segments.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                let template = name
                    .get(.."prompt:".len())
                    .filter(|prefix| prefix.eq_ignore_ascii_case("prompt:"))
                    .and_then(|_| name.get("prompt:".len()..))
                    .and_then(|template_name| {
                        crate::prompt_templates::find(
                            &self.info.runtime.prompt_templates,
                            template_name,
                        )
                    });
                if let Some(template) = template {
                    let prompt = crate::prompt_templates::expand(template, &trailing_prompt);
                    turn = TurnPrompt::standard(prompt.clone(), prompt);
                } else {
                    match self.skill_command_action(
                        &name,
                        turn.model,
                        turn.display,
                        agent.has_tool("skill"),
                    )? {
                        skill_actions::SkillCommandAction::Prompt(prompt) => turn = prompt,
                        skill_actions::SkillCommandAction::Rejected => return Ok(()),
                        skill_actions::SkillCommandAction::NotSkill => {
                            self.report_unknown_command(&name);
                            return Ok(());
                        }
                    }
                }
            }
        }

        let images = std::mem::take(&mut self.pending_images);
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
        let turn = self.prepare_goal_resumption_turn(turn);
        let mut outcome = self.run_prompt_turn(turn, images, terminal, agent).await?;
        self.finish_goal_resumption_turn(outcome.kind());
        let mut pending_goal_retries = VecDeque::new();
        let final_outcome = loop {
            let outcome_kind = outcome.kind();
            let resume_goal = goal_command::should_resume_goal_after_turn(
                outcome_kind,
                self.goal.as_ref().map(GoalState::loop_state),
                self.should_quit,
            );
            if let TurnOutcome::Failed(failed_turn) = outcome {
                if resume_goal {
                    pending_goal_retries.push_back(failed_turn);
                }
            }

            let should_drain_queue =
                goal_command::should_drain_queued_prompts(outcome_kind, resume_goal);
            if self.should_quit || !should_drain_queue {
                break outcome_kind;
            }
            let Some(prompt) = self.queued_prompts.pop_front() else {
                break outcome_kind;
            };
            self.pending_input_changed();
            self.select_pending_recall_target();
            outcome = self
                .run_prompt_turn(
                    TurnPrompt::standard(prompt.prompt, prompt.display_prompt),
                    Vec::new(),
                    terminal,
                    agent,
                )
                .await?;
        };
        if goal_command::should_resume_goal_after_turn(
            final_outcome,
            self.goal.as_ref().map(GoalState::loop_state),
            self.should_quit,
        ) {
            self.continue_goal(terminal, agent, pending_goal_retries)
                .await?;
        }
        Ok(())
    }

    async fn report_resting_herdr_state(&self) {
        let goal_blocked_reason = self
            .goal
            .as_ref()
            .filter(|goal| goal.is_blocked())
            .and_then(|goal| goal.last_reason.as_deref());
        let message = self
            .info
            .services
            .auth_unavailable
            .as_deref()
            .or(goal_blocked_reason);
        let state = if message.is_some() {
            HerdrState::Blocked
        } else {
            HerdrState::Idle
        };
        self.info
            .services
            .herdr
            .report_state(state, message, self.info.session.session_id.as_deref())
            .await;
    }

    fn paste_clipboard_image(&mut self) {
        if self.running {
            self.notify_status("image paste is unavailable while a model turn is running");
            return;
        }
        if !matches!(self.composer, ComposerMode::Input) {
            self.notify_status("image paste is only available in the message box");
            return;
        }
        match read_clipboard_image() {
            Ok(image) => {
                let summary = image_summary(&image);
                self.pending_images.push(image);
                self.notify_status(format!(
                    "attached image {} ({summary})",
                    self.pending_images.len()
                ));
            }
            Err(err) => {
                self.notify_status(format!("image paste failed: {err}"));
            }
        }
    }

    fn insert_paste(&mut self, text: &str) {
        match &mut self.composer {
            ComposerMode::Input => self.insert_pasted_input_text(text),
            ComposerMode::SecretInput(secret) => secret.insert_text(text),
            ComposerMode::ConfigNumberInput(input) => input.insert_text(text),
            ComposerMode::ConfigTextInput(input) => input.insert_text(text),
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.insert_text(text);
            }
            ComposerMode::Approval(_) | ComposerMode::Picker(_) | ComposerMode::OAuthPending(_) => {
            }
        }
    }

    fn handle_key_during_turn(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.handle_paste_burst_key(key) {
            return Ok(());
        }

        if self.handle_pending_input_key(key) {
            return Ok(());
        }

        if self.handle_approval_key(key, terminal.size()?.width as usize)?
            || self.handle_history_key(key, terminal)?
        {
            return Ok(());
        }

        if self.handle_questionnaire_key(key)? {
            return Ok(());
        }
        if self.handle_running_config_number_key(key, terminal)? {
            return Ok(());
        }
        if self.handle_running_config_text_key(key)? {
            return Ok(());
        }
        if self.handle_running_picker_key(key)? {
            return Ok(());
        }
        if self.handle_running_command_palette_key(key, terminal)? {
            return Ok(());
        }
        if self.handle_running_file_palette_key(key)? {
            return Ok(());
        }
        if self.handle_configurable_running_key(key, terminal)? {
            return Ok(());
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    self.input.clear();
                    self.paste_segments.clear();
                    self.input_submission_mode = InputSubmissionMode::ParseCommands;
                    self.pending_images.clear();
                    self.input_cursor = 0;
                    self.clamp_command_selection();
                    self.notify_status("input cleared; press esc to interrupt model");
                    self.ctrl_c_streak = 1;
                } else {
                    self.should_quit = true;
                }
            }
            (KeyModifiers::ALT, KeyCode::Backspace) => {
                self.delete_word_before_cursor();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Backspace) => {
                self.backspace_input();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Delete) => {
                self.delete_input();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Left) => {
                self.input_cursor = previous_word_boundary(&self.input, self.input_cursor);
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Right) => {
                self.input_cursor = next_word_boundary(&self.input, self.input_cursor);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Left) => {
                self.move_input_cursor_left();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Right) => {
                self.move_input_cursor_right();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Up) => {
                let width = terminal.size()?.width as usize;
                self.recall_input_history_or_move_cursor(HistoryDirection::Previous, width);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Down) => {
                let width = terminal.size()?.width as usize;
                self.recall_input_history_or_move_cursor(HistoryDirection::Next, width);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Home) => {
                self.reset_input_history_navigation();
                self.input_cursor = 0;
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::End) => {
                self.reset_input_history_navigation();
                self.input_cursor = self.input_char_len();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Enter) => {
                self.queue_prompt_after_turn()?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_input_char('\n');
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Enter) => {
                self.submit_during_turn(terminal)?;
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_input_char(ch);
                self.ctrl_c_streak = 0;
            }
            _ => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
        }
        self.clamp_command_selection();
        self.clamp_file_selection();
        Ok(())
    }

    fn submit_during_turn(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let prompt = self.expanded_input().trim().to_string();
        let display_prompt = self.input.clone();
        let paste_segments = self.paste_segments.clone();
        if prompt.is_empty() {
            self.input.clear();
            self.paste_segments.clear();
            self.input_cursor = 0;
            self.clamp_command_selection();
            return Ok(());
        }
        if let Some((mode, command)) = InlineShellMode::parse(self.input.trim()) {
            if !self.paste_segments.is_empty() {
                return self.block_pasted_inline_shell();
            }
            let command = command.to_string();
            self.clear_submitted_input();
            self.start_inline_shell(mode, command)?;
            return Ok(());
        }

        match self.parse_input_command() {
            Ok(Some(invocation)) => {
                self.input.clear();
                self.paste_segments.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                self.execute_command_during_turn(invocation, terminal)?;
            }
            Ok(None) => {
                self.queue_steering_prompt(prompt, display_prompt, paste_segments)?;
            }
            Err(commands::CommandParseError::Unknown(name)) => {
                self.input.clear();
                self.paste_segments.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                self.insert_entry(&Entry::Error(format!(
                    "unknown or unavailable command '/{name}' while a model turn is running"
                )));
                self.status = "command unavailable while running".into();
            }
        }
        Ok(())
    }

    fn queue_steering_prompt(
        &mut self,
        prompt: String,
        display_prompt: String,
        paste_segments: Vec<PasteSegment>,
    ) -> anyhow::Result<()> {
        self.reset_input_history_navigation();
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
        self.steering_prompts.push_back(QueuedPrompt {
            prompt,
            display_prompt,
            paste_segments,
        });
        self.select_pending_recall_target();
        self.insert_entry(&Entry::Notice(format!(
            "queued steer {} for after the current assistant turn",
            self.steering_prompts.len()
        )));
        self.status = format!("queued {} steer(s)", self.steering_prompts.len());
        Ok(())
    }

    fn queue_prompt_after_turn(&mut self) -> anyhow::Result<()> {
        let prompt = self.expanded_input().trim().to_string();
        let display_prompt = self.input.clone();
        let paste_segments = self.paste_segments.clone();
        if prompt.is_empty() {
            self.input.clear();
            self.paste_segments.clear();
            self.input_cursor = 0;
            self.clamp_command_selection();
            return Ok(());
        }
        self.queue_prompt(prompt, display_prompt, paste_segments)
    }

    fn queue_prompt(
        &mut self,
        prompt: String,
        display_prompt: String,
        paste_segments: Vec<PasteSegment>,
    ) -> anyhow::Result<()> {
        self.reset_input_history_navigation();
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
        self.queued_prompts.push_back(QueuedPrompt {
            prompt,
            display_prompt,
            paste_segments,
        });
        self.select_pending_recall_target();
        self.insert_entry(&Entry::Notice(format!(
            "queued message {} for after the current turn",
            self.queued_prompts.len()
        )));
        self.status = format!("queued {} message(s)", self.queued_prompts.len());
        Ok(())
    }

    fn execute_model_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        let model = invocation.args.trim();
        if model.is_empty() {
            self.refresh_available_auths();
            let picker = model_picker::model_picker_during_run(
                &self.info.runtime,
                self.pending_model_selection
                    .as_ref()
                    .map(|pending| &pending.selection),
                &self.available_auths,
            );
            if picker.items.is_empty() {
                self.insert_entry(&Entry::Notice(
                    "no cached API models. refresh model lists from /config after the current run ends."
                        .into(),
                ));
                self.status = "running".into();
            } else {
                self.composer = ComposerMode::Picker(picker);
                self.status = "select model for next turn".into();
            }
            return Ok(());
        }

        self.refresh_available_auths();
        match self.resolve_model_selection(
            model,
            &self.info.runtime.provider,
            &self.info.runtime.auth,
        ) {
            Ok(selection) => self.queue_model_selection(selection),
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "model switch failed".into();
                Ok(())
            }
        }
    }

    fn queue_model_selection(
        &mut self,
        selection: InteractiveModelSelection,
    ) -> anyhow::Result<()> {
        let provider_model = format!(
            "{}/{}",
            selection.selection.provider, selection.selection.model
        );
        self.pending_model_selection = Some(selection);
        self.insert_entry(&Entry::Notice(format!(
                "model change to {provider_model} queued; the current agent run will finish on its existing model, and the change will apply after the full run ends"
            )),
        );
        self.status = format!("model queued: {provider_model}");
        Ok(())
    }

    fn apply_pending_model_selection(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(pending) = self.pending_model_selection.take() else {
            return Ok(());
        };
        self.select_model(pending, agent)
    }

    fn execute_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Exit => self.execute_exit_command(),
            CommandId::Config => self.execute_config_command(terminal),
            CommandId::Info => self.execute_info_command(),
            CommandId::Skills => self.execute_skills_command(),
            CommandId::Agents => self.execute_agents_command(),
            CommandId::Diff => self.execute_diff_command(),
            CommandId::Doctor => self.execute_doctor_command(),
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::Goal => self.execute_goal_command_during_turn(invocation),
            CommandId::Model => self.execute_model_command_during_turn(invocation),
            CommandId::Limits => {
                self.start_limits_command();
                Ok(())
            }
            CommandId::New
            | CommandId::Compact
            | CommandId::Login
            | CommandId::Logout
            | CommandId::Resume
            | CommandId::Tree => {
                self.insert_entry(&Entry::Notice(format!(
                    "/{} is unavailable while a model turn is running",
                    invocation.name
                )));
                self.status = "command unavailable while running".into();
                Ok(())
            }
        }
    }

    fn handle_running_command_palette_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !self.command_palette_visible() {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.command_selection = if self.command_selection == 0 {
                        matches.len() - 1
                    } else {
                        self.command_selection - 1
                    };
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.command_selection = (self.command_selection + 1) % matches.len();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.command_palette_dismissed = false;
                    self.clamp_command_selection();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.clamp_command_selection();
                }
                self.submit_during_turn(terminal)?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.command_palette_dismissed = true;
                self.command_selection = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn handle_running_picker_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::Picker(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.select_previous();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.select_next();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.complete_filter();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.pop_filter_char();
                }
                Ok(true)
            }
            (KeyModifiers::CONTROL, KeyCode::Char('p')) if self.model_picker_is_open() => {
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Char(' ')) if self.picker_space_confirms_selection() => {
                self.submit_picker_selection_during_turn()?;
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.push_filter_char(ch);
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.submit_picker_selection_during_turn()?;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.handle_picker_escape(/*running*/ true)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn submit_picker_selection_during_turn(&mut self) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            self.composer = ComposerMode::Input;
            self.status = "running".into();
            return Ok(());
        };

        let return_picker = self.take_picker_parent_after_selection(action);
        if !matches!(action, PickerAction::Config) {
            self.composer = ComposerMode::Input;
        }
        match action {
            PickerAction::InsertSkillCommand => {
                self.input = format!("/skill:{value}");
                self.input_cursor = self.input_char_len();
                self.command_palette_dismissed = true;
                self.status = "skill command inserted".into();
            }
            PickerAction::ResumeSession | PickerAction::SelectTreeNode => {
                self.insert_entry(&Entry::Notice(
                    "session navigation is unavailable while a model turn is running".into(),
                ));
                self.status = "session navigation unavailable while running".into();
            }
            PickerAction::Config => self.submit_config_selection_during_turn(&value)?,
            PickerAction::Doctor | PickerAction::ViewAgent => {
                self.status = "running".into();
            }
            PickerAction::SelectInternalAgentModel => {
                self.insert_entry(&Entry::Notice(
                    "internal agent model changes are unavailable while a model turn is running"
                        .into(),
                ));
                self.status = "internal agent model change unavailable while running".into();
            }
            PickerAction::SelectModel => {
                self.refresh_available_auths();
                match self.resolve_model_selection(
                    &value,
                    &self.info.runtime.provider,
                    &self.info.runtime.auth,
                ) {
                    Ok(selection) => self.queue_model_selection(selection)?,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "model switch failed".into();
                    }
                }
            }
            PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::RefreshModelList => {
                self.insert_entry(&Entry::Notice(
                    "that picker action is unavailable while a model turn is running".into(),
                ));
                self.status = "picker action unavailable while running".into();
            }
        }
        if let Some((picker, selected_value)) = return_picker {
            self.open_main_config_picker(selected_value, picker.filter)?;
        }
        Ok(())
    }

    fn submit_config_selection_during_turn(&mut self, value: &str) -> anyhow::Result<()> {
        match value {
            value if config_picker::is_category(value) => {
                self.open_config_category(value)?;
            }
            config_picker::CONVERSATION_MODEL_VALUE => {
                self.open_config_conversation_model_picker_during_turn();
            }
            config_picker::REFRESH_MODEL_LIST_VALUE
            | config_picker::PROVIDER_LOGIN_VALUE
            | config_picker::PROVIDER_LOGOUT_VALUE => {
                self.insert_entry(&Entry::Notice(
                    "provider configuration is unavailable while a model turn is running".into(),
                ));
                self.status = "config action unavailable while running".into();
            }
            config_picker::PERMISSION_MODE_VALUE => {
                self.reject_permission_mode_change();
            }
            value if value.starts_with(config_picker::PERMISSION_MODE_PREFIX) => {
                self.reject_permission_mode_change();
            }
            config_picker::MAX_OUTPUT_BYTES_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxOutputBytes,
                    config.max_output_bytes,
                ));
                self.status = "edit max output bytes".into();
            }
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxToolOutputLines,
                    config.max_tool_output_lines,
                ));
                self.status = "edit max tool output lines".into();
            }
            config_picker::REASONING_VALUE => {
                self.insert_entry(&Entry::Notice(
                    "reasoning changes are unavailable while a model turn is running".into(),
                ));
                self.status = "config action unavailable while running".into();
            }
            config_picker::SHOW_REASONING_OUTPUT_VALUE => {
                self.toggle_reasoning_output()?;
            }
            config_picker::CHECK_FOR_UPDATES_VALUE => {
                self.toggle_check_for_updates()?;
            }
            config_picker::ENABLE_SUBAGENTS_VALUE => {
                self.toggle_enable_subagents()?;
            }
            config_picker::AUTO_COMPACT_VALUE => {
                self.toggle_auto_compact()?;
            }
            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactThresholdPercent,
                    config.compact_threshold_percent as usize,
                ));
                self.status = "edit compact threshold percent".into();
            }
            config_picker::COMPACT_TARGET_PERCENT_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactTargetPercent,
                    config.compact_target_percent as usize,
                ));
                self.status = "edit compact target percent".into();
            }
            config_picker::INLINE_SHELL_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(config_picker::inline_shell_picker(&config));
                self.status = "select inline shell".into();
            }
            value if value.starts_with(config_picker::INLINE_SHELL_PREFIX) => {
                let shell = value[config_picker::INLINE_SHELL_PREFIX.len()..].to_string();
                self.info.services.config_repository.update(|config| {
                    config.inline_shell.clone_from(&shell);
                })?;
                self.open_main_config_picker_selected(config_picker::INLINE_SHELL_VALUE)?;
                self.status = format!("inline shell: {shell}");
            }
            config_picker::WEB_SEARCH_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(config_picker::web_search_config_picker(
                    &config,
                    self.credential_store.as_ref(),
                ));
                self.status = "web search config".into();
            }
            config_picker::WEB_SEARCH_PROVIDER_VALUE => self.cycle_web_search_provider()?,
            config_picker::WEB_SEARCH_OPENAI_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::OpenAiSearch)?;
            }
            config_picker::WEB_SEARCH_EXA_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::Exa)?;
            }
            config_picker::WEB_SEARCH_BRAVE_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::Brave)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_running_config_number_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigNumberInput(_)) {
            return Ok(false);
        }
        self.handle_config_number_key(key, terminal)
    }

    fn handle_running_config_text_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigTextInput(_)) {
            return Ok(false);
        }
        self.handle_config_text_key(key)
    }

    fn reset_streams(&mut self) {
        self.assistant_stream.reset();
        self.assistant_stream_code_fence = CodeFenceState::default();
        self.reasoning_stream.reset();
        self.reasoning_stream_code_fence = CodeFenceState::default();
        self.current_stream_kind = None;
        self.stream_preview_deadline = None;
        self.live_stream_preview = None;
        self.hidden_reasoning_active = false;
    }

    fn reset_provider_attempt_stream(&mut self) {
        self.reset_streams();
        self.tool_calls.clear();
        if let Some(start) = self.provider_attempt.reset_output(&mut self.transcript) {
            self.markdown_images.clear();
            self.mark_markdown_images_dirty_from(start);
            self.history_lines.invalidate_from(start);
        }
        self.status = "retrying provider response".into();
    }

    fn activity_status(&self) -> Option<ActivityStatus> {
        let phase = match self.composer {
            ComposerMode::Approval(_) => ActivityPhase::WaitingForApproval,
            ComposerMode::Questionnaire(_) => ActivityPhase::WaitingForInput,
            _ => self.activity_phase,
        };
        ActivityStatus::from_parent_and_subagents(
            self.loading_active().then_some(phase),
            self.subagent_panel.count(),
        )
    }

    fn update_subagent_panel(&mut self, agent: &InteractiveRuntime) -> bool {
        let changed = self.subagent_panel.update(agent.subagents());
        if self.subagent_panel.is_active() {
            self.loading_spinner.start_if_needed();
        }
        changed
    }

    fn loading_active(&self) -> bool {
        self.running || !self.assistant_stream.is_empty() || !self.reasoning_stream.is_empty()
    }

    fn handle_queued_agent_event(
        &mut self,
        event: ViewModelEvent,
        terminal: &mut DefaultTerminal,
    ) -> Result<bool, rho_providers::model::ModelError> {
        Ok(self.handle_agent_event(event, terminal)?)
    }

    fn next_running_frame_deadline(
        &self,
        deferred_frame_deadline: Option<Instant>,
    ) -> tokio::time::Instant {
        let spinner_deadline = Instant::now() + LoadingSpinner::FRAME_INTERVAL;
        let deadline = deferred_frame_deadline.map_or(spinner_deadline, |deferred_deadline| {
            deferred_deadline.min(spinner_deadline)
        });
        let deadline = self
            .stream_preview_deadline
            .map_or(deadline, |stream_deadline| stream_deadline.min(deadline));
        let deadline = self
            .paste_burst
            .deadline()
            .map_or(deadline, |paste_deadline| paste_deadline.min(deadline));
        tokio::time::Instant::from_std(deadline)
    }

    fn handle_running_terminal_events(
        &mut self,
        first_event: Event,
        terminal: &mut DefaultTerminal,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
        input_mode: RunningInputMode,
    ) -> Result<StreamControl, rho_providers::model::ModelError> {
        let mut control = StreamControl::Continue;
        let mut next_event = Some(first_event);
        for _ in 0..MAX_TERMINAL_EVENTS_PER_TICK {
            let event = match next_event.take() {
                Some(event) => event,
                None => {
                    let event = self
                        .terminal_events
                        .as_mut()
                        .expect("terminal events initialized")
                        .try_next();
                    let Some(event) = event else {
                        break;
                    };
                    event?
                }
            };
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    self.text_selection = None;
                    if key.code == KeyCode::Esc
                        && matches!(self.composer, ComposerMode::Approval(_))
                    {
                        self.handle_approval_key(key, 1).map_err(|error| {
                            rho_providers::model::ModelError::InvalidResponse(error.to_string())
                        })?;
                        self.cancel_inline_shells();
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                    if key.code == KeyCode::Esc && self.cancel_inline_shells() {
                        continue;
                    }
                    if key.code == KeyCode::Esc && !self.running_escape_has_overlay_target() {
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                    if input_mode == RunningInputMode::Turn {
                        self.handle_key_during_turn(key, terminal).map_err(|err| {
                            rho_providers::model::ModelError::InvalidResponse(err.to_string())
                        })?;
                        if self.pending_input_action.is_some() {
                            break;
                        }
                    }
                    if self.should_quit {
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                }
                Event::Paste(text) if input_mode == RunningInputMode::Turn => {
                    let text = normalize_paste(&text);
                    self.flush_pending_paste_burst();
                    self.insert_paste(&text);
                    self.paste_burst.clear();
                }
                Event::Resize(_, _) => {
                    self.flush_pending_paste_burst();
                    self.text_selection = None;
                    self.hovered_code_block_copy = None;
                    self.hide_history_scrollbar();
                    self.clamp_history_scroll_for_terminal(terminal)?;
                    self.drain_streams(terminal)?;
                    control = StreamControl::Resize;
                }
                Event::Mouse(mouse) if input_mode == RunningInputMode::Turn => {
                    self.handle_mouse_event(mouse.kind, mouse.column, mouse.row, terminal)?;
                }
                Event::FocusGained => {
                    mouse_capture::reassert();
                    self.statusline.refresh_git_branch();
                }
                _ => {}
            }
        }
        self.flush_due_paste_burst();
        Ok(control)
    }

    fn running_escape_has_overlay_target(&self) -> bool {
        self.command_palette_visible()
            || self.file_palette_visible()
            || self.pending_input_focused()
            || !matches!(self.composer, ComposerMode::Input)
    }

    fn request_running_interrupt(
        &mut self,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
    ) -> StreamControl {
        interrupt_requested.store(true, Ordering::SeqCst);
        if tool_call_active.load(Ordering::SeqCst) {
            self.status = "interrupting tool".into();
        }
        StreamControl::Interrupt
    }

    fn handle_agent_event<B: Backend>(
        &mut self,
        event: ViewModelEvent,
        terminal: &mut Terminal<B>,
    ) -> Result<bool, B::Error> {
        if let Some(phase) = event.activity_phase() {
            self.activity_phase = phase;
        }
        match event {
            ViewModelEvent::ProviderStreamReset => {
                self.reset_provider_attempt_stream();
                Ok(true)
            }
            ViewModelEvent::OutputDelta(text) => {
                self.hidden_reasoning_active = false;
                let switched = self.switch_stream_kind(StreamKind::Assistant);
                self.assistant_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Assistant)?;
                self.update_stream_preview_deadline(StreamKind::Assistant);
                Ok(switched || drained)
            }
            ViewModelEvent::ReasoningDelta(text) => {
                if !self.active_turn_show_reasoning_output {
                    self.hidden_reasoning_active = true;
                    return Ok(true);
                }
                let switched = self.switch_stream_kind(StreamKind::Reasoning);
                self.reasoning_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Reasoning)?;
                self.update_stream_preview_deadline(StreamKind::Reasoning);
                Ok(switched || drained)
            }
            other => {
                if matches!(
                    other,
                    ViewModelEvent::StepStarted(_)
                        | ViewModelEvent::ToolCallUpdated { .. }
                        | ViewModelEvent::ToolStarted { .. }
                        | ViewModelEvent::ToolFinished { .. }
                ) {
                    self.hidden_reasoning_active = false;
                    self.finish_streams();
                }
                if let Some(entry) = self.record_agent_event(other) {
                    self.insert_entry(&entry);
                }
                self.drain_streams(terminal)?;
                Ok(true)
            }
        }
    }

    fn switch_stream_kind(&mut self, kind: StreamKind) -> bool {
        let inserted = if self
            .current_stream_kind
            .is_some_and(|current| current != kind)
        {
            self.finish_current_stream()
        } else {
            false
        };
        self.current_stream_kind = Some(kind);
        self.update_stream_preview_deadline(kind);
        inserted
    }

    fn drain_streams<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<bool, B::Error> {
        let reasoning_drained = self.drain_stream(terminal, StreamKind::Reasoning)?;
        let assistant_drained = self.drain_stream(terminal, StreamKind::Assistant)?;
        Ok(reasoning_drained || assistant_drained)
    }

    fn drain_stream<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        kind: StreamKind,
    ) -> Result<bool, B::Error> {
        let width = terminal.size()?.width as usize;
        let inner_width = padded_content_width(width);
        let fragment = match kind {
            StreamKind::Assistant => self
                .assistant_stream
                .drain_renderable_markdown(inner_width, self.assistant_stream_code_fence.is_open()),
            StreamKind::Reasoning => self
                .reasoning_stream
                .drain_renderable_markdown(inner_width, self.reasoning_stream_code_fence.is_open()),
        };
        if let Some(fragment) = fragment {
            self.live_stream_preview = None;
            self.insert_stream_fragment(fragment, kind);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn finish_streams(&mut self) -> bool {
        let reasoning_finished = self.finish_stream(StreamKind::Reasoning);
        let assistant_finished = self.finish_stream(StreamKind::Assistant);
        self.current_stream_kind = None;
        self.stream_preview_deadline = None;
        self.live_stream_preview = None;
        reasoning_finished || assistant_finished
    }

    fn finish_current_stream(&mut self) -> bool {
        self.current_stream_kind
            .is_some_and(|kind| self.finish_stream(kind))
    }

    fn finish_stream(&mut self, kind: StreamKind) -> bool {
        let fragment = match kind {
            StreamKind::Assistant => self.assistant_stream.finish(),
            StreamKind::Reasoning => self.reasoning_stream.finish(),
        };
        self.update_stream_preview_deadline(kind);
        if let Some(fragment) = fragment {
            self.live_stream_preview = None;
            self.insert_stream_fragment(fragment, kind);
            true
        } else {
            false
        }
    }

    fn drain_stream_preview(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<bool> {
        if self
            .stream_preview_deadline
            .is_none_or(|deadline| Instant::now() < deadline)
        {
            return Ok(false);
        }
        let Some(kind) = self.current_stream_kind else {
            self.stream_preview_deadline = None;
            return Ok(false);
        };
        let width = terminal.size()?.width as usize;
        let inner_width = padded_content_width(width);
        let preview = match kind {
            StreamKind::Assistant => self
                .assistant_stream
                .drain_preview_markdown(inner_width, self.assistant_stream_code_fence.is_open()),
            StreamKind::Reasoning => self
                .reasoning_stream
                .drain_preview_markdown(inner_width, self.reasoning_stream_code_fence.is_open()),
        };
        self.stream_preview_deadline = None;
        self.update_stream_preview_deadline(kind);
        if let Some(preview) = preview {
            self.live_stream_preview = Some(LiveStreamPreview {
                kind,
                text: preview.render_text().to_string(),
                include_leading_blank: preview.include_leading_blank(),
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn update_stream_preview_deadline(&mut self, kind: StreamKind) {
        let pending_chars = match kind {
            StreamKind::Assistant => self.assistant_stream.pending_text().chars().count(),
            StreamKind::Reasoning => self.reasoning_stream.pending_text().chars().count(),
        };
        if pending_chars < STREAM_PREVIEW_MIN_CHARS {
            self.stream_preview_deadline = None;
        } else if self.stream_preview_deadline.is_none() {
            self.stream_preview_deadline = Some(Instant::now() + STREAM_PREVIEW_DELAY);
        }
    }

    fn insert_final_answer_suffix(&mut self, answer: &str) {
        match final_answer_delta(self.assistant_stream.emitted_text(), answer) {
            FinalAnswerDelta::None => {}
            FinalAnswerDelta::Append(suffix) => {
                self.assistant_stream.push_delta(suffix);
                if let Some(fragment) = self.assistant_stream.finish() {
                    self.insert_stream_fragment(fragment, StreamKind::Assistant);
                }
            }
            FinalAnswerDelta::Mismatch => {
                self.replace_current_turn_assistant_transcript(answer);
            }
        }
    }

    fn insert_stream_fragment(&mut self, fragment: StreamFragment, kind: StreamKind) {
        let render_text = fragment.render_text();
        if !render_text.is_empty() {
            let code_fence = match kind {
                StreamKind::Assistant => &mut self.assistant_stream_code_fence,
                StreamKind::Reasoning => &mut self.reasoning_stream_code_fence,
            };
            update_code_block_state(render_text, code_fence);
            self.last_inserted_was_tool = false;
        }
        let text = fragment.into_text();
        self.push_transcript_entry(kind.entry(text));
    }

    fn replace_current_turn_assistant_transcript(&mut self, answer: &str) {
        let start = self.current_turn_start.unwrap_or(0);
        let assistant_indices = self
            .transcript
            .iter()
            .enumerate()
            .skip(start)
            .filter_map(|(index, entry)| matches!(entry, Entry::Assistant(_)).then_some(index))
            .collect::<Vec<_>>();

        let Some((first, stale)) = assistant_indices.split_first() else {
            self.push_transcript_entry(Entry::Assistant(answer.to_string()));
            return;
        };

        if let Entry::Assistant(text) = &mut self.transcript[*first] {
            *text = answer.to_string();
        }
        self.markdown_images.clear();
        self.mark_markdown_images_dirty_from(*first);
        self.history_lines.invalidate_from(*first);
        for index in stale.iter().rev() {
            self.transcript.remove(*index);
        }
    }

    fn insert_runtime_notices(&mut self, agent: &mut InteractiveRuntime) {
        for notice in agent.take_notices() {
            self.insert_entry(&Entry::Notice(notice));
        }
    }

    fn execute_config_command(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        self.info.runtime.max_tool_output_lines = config.max_tool_output_lines.max(1);
        self.info
            .services
            .diagnostics
            .update_max_tool_output_lines(self.info.runtime.max_tool_output_lines);
        self.info.runtime.show_reasoning_output = config.show_reasoning_output;
        self.composer =
            ComposerMode::Picker(config_picker::config_picker(&self.info.runtime, &config));
        self.status = "config".into();
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }

    fn toggle_latest_tool_output(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        if let Some(pending) = self.tool_calls.latest_mut() {
            if tool_display_line_count(&pending.display_lines)
                <= self.info.runtime.max_tool_output_lines
            {
                self.status = "no truncated tool output".into();
                return Ok(());
            }
            pending.expanded = !pending.expanded;
            self.status = if pending.expanded {
                "tool output expanded".into()
            } else {
                "tool output collapsed".into()
            };
            return Ok(());
        }

        let Some(index) = self.transcript.iter().rposition(|entry| {
            expandable_tool_entry(entry, self.info.runtime.max_tool_output_lines)
        }) else {
            self.status = "no truncated tool output".into();
            return Ok(());
        };

        self.toggle_transcript_tool_output(index);
        self.clamp_history_scroll_for_terminal(terminal)
    }

    fn toggle_transcript_tool_output(&mut self, index: usize) {
        let expand =
            !matches!(self.transcript.get(index), Some(Entry::Tool(tool)) if tool.expanded);
        let mut dirty_from = index;
        for (entry_index, entry) in self.transcript.iter_mut().enumerate() {
            if let Entry::Tool(tool) = entry {
                if tool.expanded {
                    dirty_from = dirty_from.min(entry_index);
                }
                tool.expanded = false;
            }
        }
        if let Some(Entry::Tool(tool)) = self.transcript.get_mut(index) {
            tool.expanded = expand;
            self.history_lines.invalidate_from(dirty_from);
        }
        self.status = if expand {
            "tool output expanded".into()
        } else {
            "tool output collapsed".into()
        };
    }

    fn reset_usage(&mut self) {
        self.cumulative_usage = None;
        self.usage_cost_tracker.reset();
        self.usage_before_current_run = None;
        self.usage_before_current_step = None;
        self.usage_before_current_attempt = None;
        self.current_run_usage = None;
        self.latest_usage = None;
    }

    fn record_agent_event(&mut self, event: ViewModelEvent) -> Option<Entry> {
        match event {
            ViewModelEvent::RunStarted => {
                self.usage_cost_tracker.run_started();
                self.usage_before_current_run = self.cumulative_usage.clone();
                self.usage_before_current_step = None;
                self.usage_before_current_attempt = None;
                self.current_run_usage = None;
                None
            }
            ViewModelEvent::StepStarted(step) => {
                self.usage_cost_tracker.step_started();
                self.usage_before_current_step = self.current_run_usage.clone();
                self.usage_before_current_attempt = None;
                self.reset_streams();
                self.provider_attempt.begin(self.transcript.len());
                self.hidden_reasoning_active = !self.active_turn_show_reasoning_output;
                self.running = true;
                self.tool_calls.clear();
                self.loading_spinner.start_if_needed();
                self.status = format!("running step {step}");
                None
            }
            ViewModelEvent::SteeringApplied(ids) => {
                self.mark_steering_applied(&ids);
                None
            }
            ViewModelEvent::ToolStarted {
                call_id,
                display_lines,
            } => {
                self.tool_calls.started(call_id, display_lines);
                None
            }
            ViewModelEvent::ToolUpdated {
                call_id,
                display_lines,
            } => {
                self.tool_calls.updated(call_id, display_lines);
                None
            }
            ViewModelEvent::ToolCallUpdated {
                index,
                call_id,
                display_lines,
            } => {
                self.tool_calls.preview(index, call_id, display_lines);
                None
            }
            ViewModelEvent::ProviderStreamReset | ViewModelEvent::ProviderRetry => {
                self.usage_cost_tracker.attempt_restarted();
                self.usage_before_current_attempt = self
                    .current_run_usage
                    .as_ref()
                    .map(|usage| usage_difference(usage, self.usage_before_current_step.as_ref()));
                None
            }
            ViewModelEvent::OutputDelta(_) | ViewModelEvent::ReasoningDelta(_) => None,
            ViewModelEvent::CompactionStarted => Some(Entry::Notice(
                event_adapter::COMPACTION_STARTED_NOTICE.into(),
            )),
            ViewModelEvent::CompactionCompleted {
                previous_messages,
                current_messages,
            } => Some(Entry::Notice(event_adapter::compaction_completed_notice(
                previous_messages,
                current_messages,
            ))),
            ViewModelEvent::ContextUsage(usage) => {
                self.info.services.diagnostics.record_context(usage.clone());
                self.current_context = Some(usage);
                None
            }
            ViewModelEvent::Usage(usage) => {
                let current_cost_source = self.usage_cost_tracker.record_usage(&usage);
                let mut current_run_usage = usage;
                if let Some(attempt_baseline) = &self.usage_before_current_attempt {
                    current_run_usage =
                        usage_with_estimated_cost(current_run_usage, self.model_metadata.as_ref());
                    let mut combined = None;
                    merge_usage(&mut combined, attempt_baseline.clone());
                    merge_usage(&mut combined, current_run_usage);
                    current_run_usage = combined.expect("attempt baseline is present");
                }
                let step_baseline = self
                    .usage_before_current_step
                    .clone()
                    .map(|usage| usage_with_estimated_cost(usage, self.model_metadata.as_ref()));
                let mut latest_usage = usage_difference(&current_run_usage, step_baseline.as_ref());
                latest_usage =
                    usage_with_estimated_cost(latest_usage, self.model_metadata.as_ref());
                if current_cost_source == CostSource::Estimated {
                    current_run_usage.cost_usd_micros = add_optional(
                        step_baseline
                            .as_ref()
                            .and_then(|usage| usage.cost_usd_micros),
                        latest_usage.cost_usd_micros,
                    );
                }
                self.current_run_usage = Some(current_run_usage.clone());
                self.latest_usage = Some(latest_usage);
                self.cumulative_usage
                    .clone_from(&self.usage_before_current_run);
                merge_usage(&mut self.cumulative_usage, current_run_usage);
                None
            }
            ViewModelEvent::ToolFinished {
                call_id,
                ok,
                display_style,
                mut display_lines,
                image_asset,
            } => {
                self.statusline.refresh_git_branch();
                let expanded = self.tool_calls.finished(&call_id);
                self.activity_phase = if self.tool_calls.is_running() {
                    ActivityPhase::RunningTool
                } else {
                    ActivityPhase::Starting
                };
                let image =
                    image_asset
                        .as_ref()
                        .and_then(|asset| match self.load_feed_image(asset) {
                            Ok(image) => image,
                            Err(error) => {
                                display_lines.push(format!("image preview unavailable: {error}"));
                                None
                            }
                        });
                Some(Entry::Tool(ToolEntry {
                    state: ToolEntryState::Finished { ok, display_style },
                    display_lines,
                    expanded,
                    image,
                }))
            }
        }
    }

    fn exit_summary(&self) -> Option<String> {
        self.info
            .session
            .session_id
            .as_ref()
            .map(|session_id| format!("rho session saved: {session_id}"))
    }

    fn insert_entry(&mut self, entry: &Entry) {
        self.record_inserted_entry(entry.clone());
    }

    fn notify_status(&mut self, status: impl Into<String>) {
        let status = status.into();
        self.status = status.clone();
        if self.last_status_notice.as_deref() == Some(status.as_str()) {
            return;
        }
        self.insert_entry(&Entry::Notice(status));
    }

    fn record_inserted_entry(&mut self, entry: Entry) {
        self.last_status_notice = match &entry {
            Entry::Notice(text) => Some(text.clone()),
            Entry::User(_)
            | Entry::Assistant(_)
            | Entry::Reasoning(_)
            | Entry::RuntimeInfo(_)
            | Entry::UsageLimits(_)
            | Entry::Tool(_)
            | Entry::Error(_) => None,
        };
        self.last_inserted_was_tool = is_tool_entry(&entry);
        self.push_transcript_entry(entry);
    }

    fn push_transcript_entry(&mut self, entry: Entry) {
        match entry {
            Entry::Assistant(text) => {
                let index = if matches!(self.transcript.last(), Some(Entry::Assistant(_))) {
                    self.transcript.len().saturating_sub(1)
                } else {
                    self.transcript.len()
                };
                match self.transcript.last_mut() {
                    Some(Entry::Assistant(previous)) => {
                        previous.push_str(&text);
                        self.history_lines.assistant_appended(index);
                    }
                    _ => {
                        self.history_lines.invalidate_from(index);
                        self.transcript.push(Entry::Assistant(text));
                    }
                }
                self.mark_markdown_images_dirty_from(index);
            }
            Entry::Reasoning(text) => match self.transcript.last_mut() {
                Some(Entry::Reasoning(previous)) => {
                    previous.push_str(&text);
                    self.history_lines
                        .invalidate_from(self.transcript.len().saturating_sub(1));
                }
                _ => {
                    self.history_lines.invalidate_from(self.transcript.len());
                    self.transcript.push(Entry::Reasoning(text));
                }
            },
            other => {
                self.last_status_notice = match &other {
                    Entry::Notice(text) => Some(text.clone()),
                    _ => None,
                };
                self.history_lines.invalidate_from(self.transcript.len());
                self.transcript.push(other);
            }
        }
    }
}

fn visible_composer_start(cursor_line: usize, line_count: usize, visible_count: usize) -> usize {
    if visible_count == 0 || visible_count >= line_count {
        return 0;
    }
    cursor_line
        .saturating_add(1)
        .saturating_sub(visible_count)
        .min(line_count.saturating_sub(visible_count))
}

fn recovered_history_tail(
    entries: &[Entry],
    width: usize,
    line_limit: usize,
    max_tool_output_lines: usize,
) -> (usize, Vec<Entry>) {
    let mut selected_start = entries.len();
    let mut line_count = 0usize;
    let mut next_is_tool = false;

    for (index, entry) in entries.iter().enumerate().rev() {
        let spacing = is_tool_entry(entry) && next_is_tool;
        let entry_line_count =
            entry_lines(entry, width, max_tool_output_lines).len() + usize::from(spacing);
        if selected_start < entries.len() && line_count + entry_line_count > line_limit {
            break;
        }
        selected_start = index;
        line_count += entry_line_count;
        next_is_tool = is_tool_entry(entry);
    }

    (selected_start, entries[selected_start..].to_vec())
}

use message_history::transcript_entries_from_messages;

fn tool_display_line_count(display_lines: &[String]) -> usize {
    display_lines
        .iter()
        .map(|line| line.lines().count().max(1))
        .sum()
}

fn text_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_message_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            ContentBlock::Image(image) => Some(format!("[image: {}]", image_summary(image))),
            ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn secret_input_lines(secret: &SecretInput, width: usize) -> Vec<Line<'static>> {
    let masked = "•".repeat(secret.value.chars().count());
    vec![
        styled_line(
            truncate_one_line(
                &format!("enter {}  enter save, esc cancel", secret.target.label),
                width,
            ),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&masked, width),
            width,
            Theme::text(),
            LineFill::Natural,
        ),
    ]
}

fn usage_with_estimated_cost(
    mut usage: ModelUsage,
    metadata: Option<&ModelMetadata>,
) -> ModelUsage {
    if usage.cost_usd_micros.is_none() {
        usage.cost_usd_micros = estimated_cost_usd_micros(&usage, metadata);
    }
    usage
}

fn usage_difference(usage: &ModelUsage, baseline: Option<&ModelUsage>) -> ModelUsage {
    let baseline = baseline.cloned().unwrap_or_default();
    ModelUsage {
        input_tokens: subtract_optional(usage.input_tokens, baseline.input_tokens),
        output_tokens: subtract_optional(usage.output_tokens, baseline.output_tokens),
        cache_read_tokens: subtract_optional(usage.cache_read_tokens, baseline.cache_read_tokens),
        cache_write_tokens: subtract_optional(
            usage.cache_write_tokens,
            baseline.cache_write_tokens,
        ),
        total_tokens: subtract_optional(usage.total_tokens, baseline.total_tokens),
        context_window: usage.context_window,
        cost_usd_micros: subtract_optional(usage.cost_usd_micros, baseline.cost_usd_micros),
    }
}

fn subtract_optional(value: Option<u64>, baseline: Option<u64>) -> Option<u64> {
    value.map(|value| value.saturating_sub(baseline.unwrap_or_default()))
}

fn merge_usage(total: &mut Option<ModelUsage>, mut usage: ModelUsage) {
    usage.total_tokens = usage.total_tokens.or_else(|| usage_total_tokens(&usage));
    let Some(total) = total.as_mut() else {
        *total = Some(usage);
        return;
    };
    total.input_tokens = add_optional(total.input_tokens, usage.input_tokens);
    total.output_tokens = add_optional(total.output_tokens, usage.output_tokens);
    total.cache_read_tokens = add_optional(total.cache_read_tokens, usage.cache_read_tokens);
    total.cache_write_tokens = add_optional(total.cache_write_tokens, usage.cache_write_tokens);
    total.total_tokens = add_optional(total.total_tokens, usage.total_tokens);
    total.cost_usd_micros = add_optional(total.cost_usd_micros, usage.cost_usd_micros);
    total.context_window = usage.context_window.or(total.context_window);
}

fn usage_total_tokens(usage: &ModelUsage) -> Option<u64> {
    let total = usage
        .total_input_tokens()
        .unwrap_or_default()
        .saturating_add(usage.output_tokens.unwrap_or_default());
    (total > 0).then_some(total)
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn oauth_pending_lines(target: &LoginTarget, width: usize) -> Vec<Line<'static>> {
    vec![styled_line(
        truncate_one_line(
            &format!("waiting for {} OAuth login  esc cancel", target.provider),
            width,
        ),
        width,
        Theme::dim(),
        LineFill::Natural,
    )]
}

fn padded_content_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

fn pad_display_line(line: Line<'static>) -> Line<'static> {
    let edge_style = line
        .spans
        .first()
        .map(|span| span.style)
        .unwrap_or_default();
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    spans.push(Span::styled(" ", edge_style));
    spans.extend(line.spans);
    spans.push(Span::styled(" ", edge_style));
    Line::from(spans)
}

fn print_exit_summary(summary: Option<&str>) -> std::io::Result<()> {
    let Some(summary) = summary else {
        return Ok(());
    };
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{summary}")?;
    stdout.flush()
}

fn previous_word_boundary(input: &str, cursor: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut index = cursor.min(chars.len());
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && !chars[index - 1].is_whitespace() {
        index -= 1;
    }
    index
}

fn next_word_boundary(input: &str, cursor: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut index = cursor.min(chars.len());
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    while index < chars.len() && !chars[index].is_whitespace() {
        index += 1;
    }
    index
}

fn enable_bracketed_paste() -> std::io::Result<()> {
    execute!(std::io::stdout(), EnableBracketedPaste)
}

fn disable_bracketed_paste() -> std::io::Result<()> {
    execute!(std::io::stdout(), DisableBracketedPaste)
}

fn enable_keyboard_enhancements() -> std::io::Result<()> {
    // On Windows, Rho reads KEY_EVENT records from ConPTY. Kitty keyboard
    // enhancements cause multiplexers such as Herdr to re-encode Shift+Tab as
    // CSI u (`\x1b[9;2u`), which ConPTY does not reverse-translate. Legacy
    // `\x1b[Z` is reverse-mapped to VK_TAB+SHIFT and reaches Rho as BackTab.
    if cfg!(windows) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "keyboard enhancements are disabled on Windows so Shift+Tab remains representable under ConPTY",
        ));
    }
    execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
}

fn disable_keyboard_enhancements() -> std::io::Result<()> {
    execute!(std::io::stdout(), PopKeyboardEnhancementFlags)
}

fn enable_modified_keys() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    // xterm mode 2 preserves modified Enter without altering printable shifted characters.
    stdout.write_all(b"\x1b[>4;2m")?;
    stdout.flush()
}

fn disable_modified_keys() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    stdout.write_all(b"\x1b[>4;0m")?;
    stdout.flush()
}

fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn paste_marker_for(text: &str) -> Option<String> {
    let line_count = text.split('\n').count();
    let char_count = text.chars().count();
    if line_count >= PASTE_COLLAPSE_MIN_LINES {
        Some(format!("[ pasted: {line_count} lines ]"))
    } else if char_count > PASTE_COLLAPSE_MIN_CHARS {
        Some(format!("[ pasted: {char_count} chars ]"))
    } else {
        None
    }
}

fn expand_paste_segments(input: &str, segments: &[PasteSegment]) -> String {
    if segments.is_empty() {
        return input.to_string();
    }

    let mut result = String::new();
    let mut cursor = 0;
    for segment in segments {
        if cursor > segment.start || segment.end() > input.chars().count() {
            continue;
        }
        result.extend(input.chars().skip(cursor).take(segment.start - cursor));
        result.push_str(&segment.content);
        cursor = segment.end();
    }
    result.extend(input.chars().skip(cursor));
    result
}

fn render_user_entry(prompt: &str, images: &[ImageContent]) -> String {
    let mut parts = Vec::new();
    if !prompt.is_empty() {
        parts.push(prompt.to_string());
    }
    parts.extend(
        images
            .iter()
            .enumerate()
            .map(|(index, image)| format!("[image {}: {}]", index + 1, image_summary(image))),
    );
    parts.join("\n")
}

fn short_session_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn slash_command_args(input: &str) -> &str {
    let token_end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    input[token_end..].trim_start()
}

fn complete_slash_command(input: &str, cursor: usize, name: &str) -> (String, usize) {
    let token_end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    let token_len = input[..token_end].chars().count();
    let args = slash_command_args(input);
    let completed = if args.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {args}")
    };
    let completed_token_len = name.chars().count() + 1;
    let new_cursor = if cursor <= token_len {
        completed_token_len
    } else {
        completed
            .chars()
            .count()
            .min(completed_token_len.saturating_add(cursor.saturating_sub(token_len)))
    };
    (completed, new_cursor)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamControl {
    Continue,
    Interrupt,
    Resize,
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
mod tests {
    use super::*;
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::{backend::TestBackend, style::Color, Terminal};
    use rho_providers::credentials::{
        save_codex_tokens, save_provider_api_key, CredentialError, CredentialResult,
        MemoryCredentialStore,
    };

    #[path = "activity_phase_tests.rs"]
    mod activity_phase_tests;
    #[path = "input_editing_tests.rs"]
    mod input_editing_tests;
    #[path = "layout_tests.rs"]
    mod layout_tests;
    #[path = "mouse_tests.rs"]
    mod mouse_tests;
    #[path = "questionnaire_interaction_tests.rs"]
    mod questionnaire_interaction_tests;
    #[path = "subagent_notification_tests.rs"]
    mod subagent_notification_tests;
    #[path = "usage_tests.rs"]
    mod usage_tests;

    #[derive(Debug)]
    struct FailingCredentialStore;

    impl CredentialStore for FailingCredentialStore {
        fn get_secret(&self, _account: &str) -> CredentialResult<Option<String>> {
            Err(CredentialError::StoreUnavailable("test failure".into()))
        }

        fn set_secret(&self, _account: &str, _secret: &str) -> CredentialResult<()> {
            unreachable!()
        }

        fn delete_secret(&self, _account: &str) -> CredentialResult<bool> {
            unreachable!()
        }
    }

    pub(super) fn test_bootstrap() -> TuiBootstrap {
        TuiBootstrap {
            runtime: RuntimeModelView {
                cwd: PathBuf::from("/tmp/project"),
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                model_aliases: Default::default(),
                reasoning: ReasoningLevel::Low,
                reasoning_source: ReasoningRequestSource::PersistedOrDefault,
                permission_mode: PermissionMode::Auto,
                show_reasoning_output: true,
                auth: "api-key".into(),
                internal_agents: Default::default(),
                favorite_models: Vec::new(),
                max_tool_output_lines: 10,
                keybindings: Keybindings::default(),
                prompt_templates: Default::default(),
            },
            session: SessionBootstrap {
                session_id: None,
                recovered_messages: Vec::new(),
                open_resume_picker: false,
            },
            services: ApplicationServices {
                config_repository: ConfigRepository::temporary_for_tests().unwrap(),
                auth_unavailable: None,
                update_notice: None,
                pending_update_notice: None,
                diagnostics: crate::diagnostics::test_diagnostics("openai", "gpt-test"),
                herdr: HerdrReporter::default(),
            },
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn buffer_row_text(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn test_tool_entry(ok: bool, display_lines: &[&str]) -> Entry {
        Entry::Tool(ToolEntry {
            state: ToolEntryState::Finished {
                ok,
                display_style: ToolDisplayStyle::file_or_command(),
            },
            display_lines: display_lines.iter().map(|line| (*line).into()).collect(),
            expanded: false,
            image: None,
        })
    }

    pub(super) fn test_app() -> App {
        let store = Arc::new(MemoryCredentialStore::default());
        save_provider_api_key(store.as_ref(), "openai", "sk-test").unwrap();
        App::new_with_credentials(test_bootstrap(), store)
    }

    #[test]
    fn info_command_uses_runtime_diagnostics() {
        let mut app = test_app();

        app.execute_info_command().unwrap();

        assert!(matches!(app.transcript.last(), Some(Entry::RuntimeInfo(_))));
        assert_eq!(app.status, "runtime info");
    }

    #[test]
    fn interrupt_during_tool_ends_turn_immediately() {
        let mut app = test_app();
        let interrupt_requested = AtomicBool::new(false);
        let tool_call_active = AtomicBool::new(true);

        let control = app.request_running_interrupt(&interrupt_requested, &tool_call_active);

        assert!(interrupt_requested.load(Ordering::SeqCst));
        assert!(matches!(control, StreamControl::Interrupt));
        assert_eq!(app.status, "interrupting tool");
    }

    #[test]
    fn sanitizes_generated_session_title() {
        assert_eq!(
            session_title::sanitize_session_title("\"Implement resume picker.\""),
            Some("Implement resume picker".into())
        );
        assert_eq!(session_title::sanitize_session_title("\n\n"), None);
    }

    #[test]
    fn title_model_defaults_to_main_model() {
        let app = test_app();

        assert_eq!(
            app.internal_agent_model_selection(crate::agent::SESSION_TITLE_AGENT_ID),
            ("openai".into(), "gpt-5.5".into(), "api-key".into())
        );
    }

    #[test]
    fn context_usage_event_is_tracked_separately_from_cumulative_usage() {
        let mut app = test_app();
        app.cumulative_usage = Some(ModelUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(500),
            ..ModelUsage::default()
        });

        assert!(app
            .record_agent_event(ViewModelEvent::ContextUsage(ContextUsage::estimated(
                250,
                Some(10_000),
            )))
            .is_none());

        assert_eq!(
            app.current_context,
            Some(ContextUsage::estimated(250, Some(10_000)))
        );
        assert_eq!(
            app.cumulative_usage
                .as_ref()
                .and_then(|usage| usage.input_tokens),
            Some(1_000)
        );
    }

    #[test]
    fn transcript_and_status_mutations_do_not_require_a_terminal() {
        let mut app = test_app();

        app.insert_entry(&Entry::Assistant("hello".into()));
        app.insert_entry(&Entry::Assistant(" world".into()));
        app.notify_status("ready");
        app.notify_status("ready");

        assert!(matches!(
            app.transcript.as_slice(),
            [Entry::Assistant(answer), Entry::Notice(status)]
                if answer == "hello world" && status == "ready"
        ));
        assert_eq!(app.status, "ready");
    }

    #[test]
    fn transcript_entries_render_without_prefix_labels() {
        let entries = [
            Entry::User("hello?".into()),
            Entry::Assistant("hi".into()),
            test_tool_entry(true, &["read_file", "read src/main.rs"]),
            Entry::Notice("note".into()),
            Entry::Error("bad".into()),
        ];

        let rendered = entries
            .iter()
            .flat_map(|entry| entry_lines(entry, 40, 10))
            .map(|line| line_text(&line))
            .collect::<Vec<_>>()
            .join("\n");

        for label in ["you>", "rho>", "reasoning>", "tool:", "notice>", "error>"] {
            assert!(
                !rendered.contains(label),
                "rendered label {label}: {rendered}"
            );
        }
    }

    #[test]
    fn recovered_history_tail_limits_initial_redraw() {
        let entries = (0..10)
            .map(|index| Entry::User(format!("message {index}")))
            .collect::<Vec<_>>();

        let (omitted, visible) = recovered_history_tail(&entries, 80, 9, 10);

        assert_eq!(omitted, 7);
        assert!(matches!(visible.as_slice(), [
            Entry::User(a),
            Entry::User(b),
            Entry::User(c),
        ] if a == "message 7" && b == "message 8" && c == "message 9"));
    }

    #[test]
    fn key_event_paste_burst_collapses_through_common_paste_path() {
        let start = Instant::now();
        let mut app = test_app();

        for (index, ch) in "alpha\nbeta".chars().enumerate() {
            let key = if ch == '\n' {
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            } else {
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
            };
            assert!(app.handle_paste_burst_key_at(key, start + Duration::from_millis(index as u64)));
        }
        app.flush_pending_paste_burst();

        assert_eq!(app.input, "[ pasted: 2 lines ]");
        assert_eq!(app.expanded_input(), "alpha\nbeta");
    }

    #[test]
    fn idle_key_event_text_is_inserted_without_paste_marker() {
        let start = Instant::now();
        let mut app = test_app();

        assert!(app.handle_paste_burst_key_at(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            start
        ));
        assert!(!app.handle_paste_burst_key_at(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            start + Duration::from_millis(20)
        ));

        assert_eq!(app.input, "a");
        assert!(app.paste_segments.is_empty());
    }

    #[test]
    fn single_character_fast_enter_is_buffered_as_paste() {
        let start = Instant::now();
        let mut app = test_app();

        assert!(app.handle_paste_burst_key_at(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            start
        ));
        assert!(app.handle_paste_burst_key_at(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            start + Duration::from_millis(1)
        ));

        assert_eq!(app.input, "");
        assert!(app.paste_segments.is_empty());
    }

    #[test]
    fn pasted_multiline_input_collapses_to_marker_and_expands() {
        let mut app = test_app();

        app.insert_pasted_input_text("alpha\nbeta\ngamma");

        assert_eq!(app.input, "[ pasted: 3 lines ]");
        assert_eq!(app.input_cursor, app.input.chars().count());
        assert_eq!(app.expanded_input(), "alpha\nbeta\ngamma");
    }

    #[test]
    fn pasted_single_line_input_stays_literal_until_large() {
        let mut app = test_app();

        app.insert_pasted_input_text("hello world");

        assert_eq!(app.input, "hello world");
        assert!(app.paste_segments.is_empty());
        assert_eq!(app.expanded_input(), "hello world");
    }

    #[test]
    fn paste_segments_shift_after_edits_before_marker() {
        let mut app = test_app();
        app.insert_pasted_input_text("alpha\nbeta");
        app.input_cursor = 0;
        app.insert_input_text("prefix ");

        assert_eq!(app.input, "prefix [ pasted: 2 lines ]");
        assert_eq!(app.expanded_input(), "prefix alpha\nbeta");
    }

    #[test]
    fn queued_pasted_prompt_keeps_marker_when_recalled_for_editing() {
        let mut app = test_app();
        app.insert_pasted_input_text("alpha\nbeta");
        let queued = QueuedPrompt {
            prompt: app.expanded_input(),
            display_prompt: app.input.clone(),
            paste_segments: app.paste_segments.clone(),
        };
        app.input.clear();
        app.paste_segments.clear();
        app.queued_prompts.push_back(queued);

        assert!(app.handle_pending_input_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT,)));
        assert_eq!(app.input, "[ pasted: 2 lines ]");
        assert_eq!(app.expanded_input(), "alpha\nbeta");
    }

    #[test]
    fn queued_pasted_prompt_preserves_leading_space_segment_offsets() {
        let mut app = test_app();
        app.insert_input_text(" ");
        app.insert_pasted_input_text("alpha\nbeta");
        let queued = QueuedPrompt {
            prompt: app.expanded_input().trim().to_string(),
            display_prompt: app.input.clone(),
            paste_segments: app.paste_segments.clone(),
        };
        app.input.clear();
        app.paste_segments.clear();
        app.queued_prompts.push_back(queued);

        assert!(app.handle_pending_input_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT,)));
        assert_eq!(app.input, " [ pasted: 2 lines ]");
        assert_eq!(app.expanded_input().trim(), "alpha\nbeta");
    }

    #[test]
    fn slash_command_args_can_keep_collapsed_display_separate_from_expanded_prompt() {
        let mut app = test_app();
        app.insert_input_text("/skill:test ");
        app.insert_pasted_input_text("alpha\nbeta");

        let expanded_input = app.expanded_input();
        assert_eq!(slash_command_args(&expanded_input).trim(), "alpha\nbeta");
        assert_eq!(slash_command_args(&app.input).trim(), "[ pasted: 2 lines ]");
    }

    #[test]
    fn normalize_paste_converts_carriage_returns() {
        assert_eq!(normalize_paste("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn recovered_session_messages_become_transcript_entries() {
        let entries = transcript_entries_from_messages(
            &[
                Message::System("system".into()),
                Message::User(vec![
                    ContentBlock::Text("hello".into()),
                    ContentBlock::Image(ImageContent {
                        data: "aW1n".into(),
                        mime_type: "image/png".into(),
                    }),
                ]),
                Message::Assistant(vec![ContentBlock::Text("hi".into())]),
                Message::Assistant(vec![ContentBlock::ToolCall(rho_tools::tool::ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                })]),
                Message::ToolResult(rho_tools::tool::ToolResult {
                    id: "call_1".into(),
                    ok: false,
                    content: "missing file".into(),
                }),
            ],
            std::path::Path::new(""),
        );

        assert!(
            matches!(entries[0], Entry::User(ref text) if text == "hello\n[image: image/png 3 B]")
        );
        assert!(matches!(entries[1], Entry::Assistant(ref text) if text == "hi"));
        assert!(matches!(
            entries[2],
            Entry::Tool(ToolEntry {
                state: ToolEntryState::Finished {
                    ok: false,
                    display_style: ToolDisplayStyle::FileOrCommand,
                },
                ref display_lines,
                ..
            }) if display_lines == &vec!["read_file src/main.rs".to_string()]
        ));
        let lines = entry_lines(&entries[2], 40, 10);
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::White));
        assert_eq!(lines[1].spans[0].style.bg, Some(Color::Red));
        assert!(!lines[1].spans[0].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn bash_tool_block_shows_command() {
        let lines = entry_lines(
            &test_tool_entry(true, &["bash", "cargo test", "ignored output"]),
            40,
            10,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("bash"));
        assert!(rendered.contains("cargo test"));
        assert!(!rendered.contains("tool:"));
    }

    #[test]
    fn read_file_tool_block_shows_file_name_only() {
        let lines = entry_lines(
            &test_tool_entry(true, &["read_file", "src/main.rs"]),
            40,
            10,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("src/main.rs"));
    }

    #[test]
    fn skill_tool_block_shows_single_magenta_status_line() {
        let lines = entry_lines(
            &Entry::Tool(ToolEntry {
                state: ToolEntryState::Finished {
                    ok: true,
                    display_style: ToolDisplayStyle::skill(),
                },
                display_lines: vec!["skill caveman".into()],
                expanded: false,
                image: None,
            }),
            40,
            10,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(lines[1].spans[0].style.fg, Some(Color::White));
        assert_eq!(lines[1].spans[0].style.bg, Some(Color::Magenta));
        assert!(!lines[1].spans[0].style.add_modifier.contains(Modifier::DIM));
        assert!(rendered.contains("skill caveman"));
        assert_eq!(rendered.matches("skill").count(), 1);
    }

    #[test]
    fn skill_tool_block_uses_subtle_red_failure_background() {
        let lines = entry_lines(
            &Entry::Tool(ToolEntry {
                state: ToolEntryState::Finished {
                    ok: false,
                    display_style: ToolDisplayStyle::skill(),
                },
                display_lines: vec!["unknown skill".into()],
                expanded: false,
                image: None,
            }),
            40,
            10,
        );

        assert_eq!(lines[1].spans[0].style.fg, Some(Color::White));
        assert_eq!(lines[1].spans[0].style.bg, Some(Color::Red));
        assert!(!lines[1].spans[0].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn read_file_tool_block_shows_line_range_label() {
        let lines = entry_lines(
            &test_tool_entry(true, &["read_file", "src/file.rs:10-24"]),
            40,
            10,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("src/file.rs:10-24"));
    }
    #[test]
    fn tool_block_truncates_multiline_output_with_expand_prompt() {
        let lines = entry_lines(
            &test_tool_entry(true, &["bash", "line 1\nline 2\nline 3"]),
            40,
            2,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("bash"));
        assert!(rendered.contains("line 1"));
        assert!(!rendered.contains("line 2"));
        assert!(rendered.contains("... 2 more lines, ctrl+o to expand"));
    }

    #[test]
    fn expanded_tool_block_shows_full_multiline_output() {
        let mut entry = test_tool_entry(true, &["bash", "line 1\nline 2\nline 3"]);
        let Entry::Tool(tool) = &mut entry else {
            panic!("expected tool entry");
        };
        tool.expanded = true;

        let lines = entry_lines(&entry, 40, 2);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("line 1"));
        assert!(rendered.contains("line 2"));
        assert!(rendered.contains("line 3"));
        assert!(rendered.contains("ctrl+o to collapse"));
    }

    #[test]
    fn untruncated_tool_block_does_not_show_expand_prompt() {
        let lines = entry_lines(&test_tool_entry(true, &["bash", "line 1"]), 40, 2);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(!rendered.contains("ctrl+o"));
    }

    #[test]
    fn toggling_latest_truncated_tool_collapses_previous_tool() {
        let mut app = test_app();
        app.info.runtime.max_tool_output_lines = 1;
        app.transcript = vec![
            test_tool_entry(true, &["first", "a\nb"]),
            test_tool_entry(true, &["second", "c\nd"]),
        ];
        if let Entry::Tool(tool) = &mut app.transcript[0] {
            tool.expanded = true;
        }

        let index = app
            .transcript
            .iter()
            .rposition(|entry| expandable_tool_entry(entry, app.info.runtime.max_tool_output_lines))
            .unwrap();
        for entry in &mut app.transcript {
            if let Entry::Tool(tool) = entry {
                tool.expanded = false;
            }
        }
        if let Entry::Tool(tool) = &mut app.transcript[index] {
            tool.expanded = true;
        }

        assert!(matches!(
            app.transcript[0],
            Entry::Tool(ToolEntry {
                expanded: false,
                ..
            })
        ));
        assert!(matches!(
            app.transcript[1],
            Entry::Tool(ToolEntry { expanded: true, .. })
        ));
    }

    #[test]
    fn final_answer_delta_handles_unstreamed_suffix_and_mismatch() {
        assert_eq!(
            final_answer_delta("", "final"),
            FinalAnswerDelta::Append("final")
        );
        assert_eq!(
            final_answer_delta("hello", "hello world"),
            FinalAnswerDelta::Append(" world")
        );
        assert_eq!(final_answer_delta("hello", "hello"), FinalAnswerDelta::None);
        assert_eq!(
            final_answer_delta("hello", "goodbye"),
            FinalAnswerDelta::Mismatch
        );
    }

    #[test]
    fn final_answer_mismatch_replaces_transcript_without_duplicating_entry() {
        let mut app = test_app();
        app.push_transcript_entry(Entry::Assistant("streamed".into()));

        app.replace_current_turn_assistant_transcript("final");

        assert!(matches!(
            app.transcript.as_slice(),
            [Entry::Assistant(text)] if text == "final"
        ));
    }

    #[test]
    fn final_answer_mismatch_replaces_transcript_with_empty_answer() {
        let mut app = test_app();
        app.push_transcript_entry(Entry::Assistant("streamed".into()));

        app.replace_current_turn_assistant_transcript("");

        assert!(matches!(
            app.transcript.as_slice(),
            [Entry::Assistant(text)] if text.is_empty()
        ));
    }

    #[test]
    fn final_answer_mismatch_replaces_interleaved_current_turn_assistant_fragments() {
        let mut app = test_app();
        app.push_transcript_entry(Entry::User("prompt".into()));
        app.current_turn_start = Some(app.transcript.len());
        app.push_transcript_entry(Entry::Assistant("hel".into()));
        app.push_transcript_entry(Entry::Reasoning("thinking".into()));
        app.push_transcript_entry(Entry::Assistant("lo".into()));

        app.replace_current_turn_assistant_transcript("goodbye");

        assert!(matches!(
            app.transcript.as_slice(),
            [Entry::User(_), Entry::Assistant(text), Entry::Reasoning(_)] if text == "goodbye"
        ));
    }

    #[test]
    fn step_started_clears_stream_state() {
        let mut app = test_app();
        app.assistant_stream.push_delta("current");
        app.reasoning_stream.push_delta("reasoning");

        assert!(app
            .record_agent_event(ViewModelEvent::StepStarted(2))
            .is_none());

        assert!(app.assistant_stream.is_empty());
        assert!(app.reasoning_stream.is_empty());
        assert!(app.running);
        assert_eq!(app.status, "running step 2");
    }

    #[test]
    fn active_lines_do_not_render_pending_stream_text() {
        let mut app = test_app();
        app.running = true;
        app.assistant_stream.push_delta("hello");
        app.reasoning_stream.push_delta("thinking");
        let lines = app.active_lines(40);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("starting"), "{rendered}");
        assert!(!rendered.contains("hello"), "{rendered}");
        assert!(!rendered.contains("thinking"), "{rendered}");
    }

    #[test]
    fn input_divider_style_tracks_reasoning_level() {
        let mut app = test_app();
        app.input = "hello".into();

        app.info.runtime.reasoning = ReasoningLevel::Off;
        let off_lines = app.active_lines(20);
        let off_divider = off_lines
            .iter()
            .find(|line| line_text(line) == "────────────────────")
            .unwrap();
        let off_style = off_divider.style;

        app.info.runtime.reasoning = ReasoningLevel::High;
        let high_lines = app.active_lines(20);
        let divider_indices = high_lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                (line_text(line) == "────────────────────").then_some(index)
            })
            .collect::<Vec<_>>();
        let input_index = high_lines
            .iter()
            .position(|line| line_text(line) == "hello")
            .unwrap();
        let composer_top_divider = divider_indices
            .iter()
            .copied()
            .find(|index| *index + 1 == input_index)
            .unwrap();
        let high_style = high_lines[composer_top_divider].style;

        assert_eq!(
            line_text(&high_lines[composer_top_divider]),
            "────────────────────"
        );
        assert_eq!(line_text(&high_lines[input_index]), "hello");
        assert_eq!(
            line_text(&high_lines[input_index + 1]),
            "────────────────────"
        );
        assert_eq!(
            off_style,
            Theme::reasoning_input_border(ReasoningLevel::Off)
        );
        assert_eq!(
            high_style,
            Theme::reasoning_input_border(ReasoningLevel::High)
        );
        assert_eq!(high_lines[input_index + 1].style, high_style);
        assert_ne!(off_style, high_style);
    }

    #[test]
    fn active_lines_for_height_uses_actual_viewport_height() {
        let mut app = test_app();
        app.running = true;

        let small_lines = app.active_lines_for_height(40, 4);
        let default_lines = app.active_lines_for_height(40, DEFAULT_TUI_HEIGHT as usize);
        let small_rendered = small_lines.iter().map(line_text).collect::<Vec<_>>().join(
            "
",
        );
        let default_rendered = default_lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join(
                "
",
            );

        assert!(!small_rendered.contains("starting"), "{small_rendered}");
        assert!(default_rendered.contains("starting"), "{default_rendered}");
    }

    #[test]
    fn spinner_is_anchored_immediately_above_composer_divider() {
        let mut app = test_app();
        app.running = true;
        app.tool_calls
            .preview(0, None, vec!["bash".into(), "cargo test".into()]);
        let width = 40;
        let height = 24;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let layout = app.screen_layout(Rect::new(0, 0, width, height), Instant::now());
        let rows = (0..height)
            .map(|row| buffer_row_text(terminal.backend().buffer(), row))
            .collect::<Vec<_>>();
        let activity = layout.activity.unwrap();
        assert_eq!(activity.y.saturating_add(1), layout.top_divider.y);
        assert_eq!(activity.y, layout.history.bottom().saturating_sub(1));
        assert!(activity.width < layout.history.width);
        assert!(rows[activity.y as usize].contains("starting"), "{rows:#?}");
        assert!(
            rows[..activity.y as usize]
                .iter()
                .any(|row| row.contains("cargo test")),
            "{rows:#?}"
        );
    }

    #[test]
    fn active_lines_hide_spinner_when_idle() {
        let mut app = test_app();
        let rendered = app
            .active_lines_at_for_height(40, DEFAULT_TUI_HEIGHT as usize, Instant::now())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("starting"), "{rendered}");
    }

    #[test]
    fn draw_anchors_last_live_line_to_viewport_bottom() {
        let mut app = test_app();
        let height = 24;
        let backend = TestBackend::new(60, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let bottom = buffer_row_text(terminal.backend().buffer(), height.saturating_sub(1));
        assert!(bottom.contains("low"), "{bottom:?}");
        assert!(!bottom.contains("ready"), "{bottom:?}");
    }

    #[test]
    fn long_input_keeps_statusline_and_cursor_visible() {
        let mut app = test_app();
        app.input = (0..30)
            .map(|index| format!("line {index:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.input_cursor = app.input_char_len();
        let height = 8;
        let backend = TestBackend::new(40, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let rows = (0..height)
            .map(|row| buffer_row_text(terminal.backend().buffer(), row))
            .collect::<Vec<_>>();
        let bottom = rows.last().unwrap();
        let cursor = terminal.backend().cursor_position();
        assert!(rows.iter().any(|row| row.contains("line 29")), "{rows:#?}");
        assert!(bottom.contains("low"), "{bottom:?}");
        assert!(!bottom.contains("ready"), "{bottom:?}");
        assert!(cursor.y < height, "{cursor:?}");
        assert!(
            rows[cursor.y as usize].contains("line 29"),
            "{rows:#?} {cursor:?}"
        );
    }

    #[test]
    fn command_palette_anchors_last_suggestion_to_viewport_bottom() {
        let mut app = test_app();
        app.input = "/m".into();
        app.input_cursor = 2;
        app.clamp_command_selection();
        let height = 24;
        let backend = TestBackend::new(60, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let bottom = buffer_row_text(terminal.backend().buffer(), height.saturating_sub(1));
        assert!(
            bottom.contains("/model") || bottom.contains("/"),
            "{bottom:?}"
        );
        assert!(!bottom.trim().is_empty(), "{bottom:?}");
    }

    #[test]
    fn long_picker_filter_does_not_clip_bottom_status() {
        let mut app = test_app();
        let mut picker = UiPicker::new(
            "models",
            "enter select",
            vec![PickerItem {
                label: "gpt-5.5".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "gpt-5.5".into(),
            }],
            PickerAction::SelectModel,
        );
        picker.filter = "x".repeat(120);
        app.composer = ComposerMode::Picker(picker);
        let height = 24;
        let backend = TestBackend::new(40, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let bottom = buffer_row_text(terminal.backend().buffer(), height.saturating_sub(1));
        assert!(bottom.contains("low"), "{bottom:?}");
        assert!(!bottom.contains("ready"), "{bottom:?}");
    }

    #[test]
    fn command_palette_visibility_tracks_leading_command_token() {
        let mut app = test_app();

        app.input = "/".into();
        app.input_cursor = 1;
        app.clamp_command_selection();
        assert!(app.command_palette_visible());

        app.input = "/mo".into();
        app.input_cursor = 3;
        app.clamp_command_selection();
        assert!(app.command_palette_visible());

        app.input = "/model arg".into();
        app.input_cursor = app.input_char_len();
        app.clamp_command_selection();
        assert!(!app.command_palette_visible());

        app.input = "hello /model".into();
        app.input_cursor = app.input_char_len();
        app.clamp_command_selection();
        assert!(!app.command_palette_visible());
    }

    #[test]
    fn file_palette_stays_inline_with_input_and_inserts_selected_path() {
        use std::fs;

        use tempfile::tempdir;

        file_picker::clear_workspace_file_path_cache();
        let workspace = tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/lib.rs"), "").unwrap();
        fs::write(workspace.path().join("README.md"), "").unwrap();

        let mut app = test_app();
        app.info.runtime.cwd = workspace.path().to_path_buf();
        app.input = "review @slr".into();
        app.input_cursor = app.input_char_len();
        app.clamp_file_selection();

        assert!(app.file_palette_visible());
        assert!(!matches!(app.composer, ComposerMode::Picker(_)));
        assert_eq!(app.selected_file_path().as_deref(), Some("src/lib.rs"));

        let rendered = app
            .active_lines(60)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("> @src/lib.rs"), "{rendered}");
        assert!(rendered.contains("review @slr"), "{rendered}");

        app.insert_selected_file_path("src/lib.rs");
        assert_eq!(app.input, "review @src/lib.rs ");
        assert!(!app.file_palette_visible());

        app.input = "review @src/lib.rs later".into();
        app.input_cursor = 11;
        app.input_changed();
        app.insert_selected_file_path("src/main.rs");
        assert_eq!(app.input, "review @src/main.rs later");
    }

    #[test]
    fn file_palette_arrow_keys_scroll_beyond_visible_window() {
        use std::fs;

        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use tempfile::tempdir;

        file_picker::clear_workspace_file_path_cache();
        let workspace = tempdir().unwrap();
        for index in 0..8 {
            fs::write(workspace.path().join(format!("file-{index}.txt")), "").unwrap();
        }

        let mut app = test_app();
        app.info.runtime.cwd = workspace.path().to_path_buf();
        app.input = "@".into();
        app.input_cursor = 1;
        app.clamp_file_selection();

        let matches = app.file_matches();
        assert!(matches.len() > MAX_COMMAND_SUGGESTIONS, "{matches:?}");

        let top_rendered = app
            .command_suggestion_lines(60)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            top_rendered.contains("↓ 3 more · 8 total"),
            "{top_rendered}"
        );
        assert!(!top_rendered.contains("↑"), "{top_rendered}");

        for _ in 0..MAX_COMMAND_SUGGESTIONS {
            app.handle_file_palette_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
                .unwrap();
        }

        assert_eq!(app.file_selection, MAX_COMMAND_SUGGESTIONS);
        assert_eq!(
            app.selected_file_path().as_deref(),
            Some(matches[MAX_COMMAND_SUGGESTIONS].as_str())
        );

        let rendered = app
            .command_suggestion_lines(60)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains(&format!("> @{}", matches[MAX_COMMAND_SUGGESTIONS])),
            "{rendered}"
        );
        assert!(
            !rendered.contains(&format!("@{}", matches[0])),
            "expected window to scroll past first match: {rendered}"
        );
        assert!(
            rendered.contains("↑ 1 more · ↓ 2 more · 8 total"),
            "{rendered}"
        );
    }

    #[test]
    fn command_palette_rendering_shows_selected_match() {
        let mut app = test_app();
        app.input = "/m".into();
        app.input_cursor = 2;
        app.clamp_command_selection();

        let rendered = app
            .active_lines(60)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("> /model [model]"), "{rendered}");
        assert!(rendered.contains("show or switch model"), "{rendered}");
    }

    #[test]
    fn command_palette_renders_under_message_box() {
        let mut app = test_app();
        app.input = "/m".into();
        app.input_cursor = 2;
        app.clamp_command_selection();

        let lines = app
            .active_lines(60)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();
        let input_index = lines
            .iter()
            .position(|line| line.trim_end() == "/m")
            .unwrap();
        let suggestion_index = lines
            .iter()
            .position(|line| line.contains("> /model [model]"))
            .unwrap();

        assert!(suggestion_index > input_index, "{lines:#?}");
    }

    #[test]
    fn picker_renders_in_place_of_message_box() {
        let mut app = test_app();
        app.input = "draft prompt".into();
        app.input_cursor = app.input_char_len();
        app.composer = ComposerMode::Picker(UiPicker::new(
            "select model",
            "enter confirm",
            vec![
                PickerItem {
                    label: "model-a".into(),
                    detail: None,
                    preview: None,
                    badge: Some(PickerBadge {
                        text: "(selected)".into(),
                        tone: PickerBadgeTone::Selected,
                    }),
                    value: "model-a".into(),
                },
                PickerItem {
                    label: "model-b".into(),
                    detail: None,
                    preview: None,
                    badge: None,
                    value: "model-b".into(),
                },
            ],
            PickerAction::SelectModel,
        ));

        let rendered = app
            .active_lines(60)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("select model"), "{rendered}");
        assert!(rendered.contains("→ model-a"), "{rendered}");
        assert!(!rendered.contains("draft prompt"), "{rendered}");
    }

    #[test]
    fn secret_input_masks_api_key() {
        let mut app = test_app();
        let target = catalog::login_target_for_provider("openai").unwrap();
        let mut secret = SecretInput::new(target);
        secret.insert_text("sk-secret-value");
        app.composer = ComposerMode::SecretInput(secret);

        let rendered = app
            .active_lines(60)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("enter OpenAI API key"), "{rendered}");
        assert!(rendered.contains("••••"), "{rendered}");
        assert!(!rendered.contains("sk-secret-value"), "{rendered}");
    }

    #[test]
    fn login_provider_picker_uses_readable_group_prompts() {
        let labels = catalog::login_groups()
            .into_iter()
            .map(|group| group.prompt.to_string())
            .collect::<Vec<_>>();
        for prompt in [
            "OpenAI",
            "Anthropic",
            "Google Gemini",
            "GitHub Copilot",
            "Moonshot AI",
            "xAI",
        ] {
            assert!(
                labels.iter().any(|label| label == prompt),
                "missing {prompt} in {labels:?}"
            );
        }

        let mut app = test_app();
        app.open_login_picker();
        let rendered = app
            .active_lines(80)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        for internal_name in ["openai-codex", "kimi-code", "xai-oauth", "api-key"] {
            assert!(!rendered.contains(internal_name), "{rendered}");
        }
    }

    #[test]
    fn login_method_picker_uses_readable_auth_prompts() {
        let picker = provider_picker::login_method_picker(catalog::login_group("xai").unwrap());
        let labels = picker
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["API Key", "OAuth"]);
    }

    #[test]
    fn logout_provider_picker_uses_only_providers_with_stored_credentials() {
        let store = MemoryCredentialStore::default();
        save_provider_api_key(&store, "openai", "sk-test").unwrap();
        save_provider_api_key(&store, "anthropic", "sk-ant-test").unwrap();

        let picker = provider_picker::logout_provider_picker(&store).unwrap();
        let values = picker
            .items
            .iter()
            .map(|item| item.value.as_str())
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["openai", "anthropic"]);
    }

    #[test]
    fn logout_provider_picker_propagates_credential_store_errors() {
        let error = provider_picker::logout_provider_picker(&FailingCredentialStore).unwrap_err();

        assert_eq!(
            error.to_string(),
            CredentialError::StoreUnavailable("test failure".into()).to_string()
        );
    }

    #[test]
    fn model_picker_uses_all_available_auths() {
        let store = Arc::new(MemoryCredentialStore::default());
        save_provider_api_key(store.as_ref(), "openai", "sk-test").unwrap();
        save_codex_tokens(
            store.as_ref(),
            &rho_providers::credentials::CodexTokens {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                id_token: None,
                account_id: None,
            },
        )
        .unwrap();
        save_provider_api_key(store.as_ref(), "anthropic", "sk-ant-test").unwrap();
        let mut app = App::new_with_credentials(test_bootstrap(), store);
        app.refresh_available_auths();

        let models = catalog::available_models_for_auths(&app.available_auths);

        assert!(app.available_auths.iter().any(|auth| auth == "api-key"));
        assert!(app
            .available_auths
            .iter()
            .any(|auth| auth == "anthropic-api-key"));
        assert!(models.iter().any(|model| model.provider == "openai-codex"));
    }

    #[test]
    fn model_picker_fuzzy_matches_and_autocompletes() {
        let mut picker = UiPicker::new(
            "select model",
            "enter confirm",
            vec![
                PickerItem {
                    label: "openai/gpt-5.5".into(),
                    detail: None,
                    preview: None,
                    badge: None,
                    value: "openai/gpt-5.5".into(),
                },
                PickerItem {
                    label: "openai-codex/gpt-5.4-mini".into(),
                    detail: None,
                    preview: None,
                    badge: None,
                    value: "openai-codex/gpt-5.4-mini".into(),
                },
            ],
            PickerAction::SelectModel,
        );

        for ch in "ocg54m".chars() {
            picker.push_filter_char(ch);
        }

        assert_eq!(picker.matching_indices(), vec![1]);
        assert_eq!(
            picker.selected_item().unwrap().value,
            "openai-codex/gpt-5.4-mini"
        );
        picker.complete_filter();
        assert_eq!(picker.filter, "openai-codex/gpt-5.4-mini");
    }

    #[test]
    fn picker_lines_render_name_detail_table_with_truncated_detail() {
        let picker = UiPicker::new(
            "loaded skills",
            "enter inserts command",
            vec![PickerItem {
                label: "test-skill".into(),
                detail: Some("this detail is much too long for the available width".into()),
                preview: None,
                badge: None,
                value: "test-skill".into(),
            }],
            PickerAction::InsertSkillCommand,
        );

        let lines = picker_lines(&picker, 36);

        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(!rendered.contains("| detail"), "{rendered}");
        assert!(rendered.contains("→ test-skill"), "{rendered}");
        assert!(
            rendered.contains("this detail is much too long"),
            "{rendered}"
        );
        assert!(rendered.contains("loaded skills"), "{rendered}");
    }

    #[test]
    fn picker_lines_use_single_column_without_details() {
        let picker = UiPicker::new(
            "select model",
            "enter confirm",
            vec![PickerItem {
                label: "openai-codex/gpt-5.3-codex-max".into(),
                detail: None,
                preview: None,
                badge: Some(PickerBadge {
                    text: "(selected)".into(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: "openai-codex/gpt-5.3-codex-max".into(),
            }],
            PickerAction::SelectModel,
        );

        let lines = picker_lines(&picker, 60);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(!rendered.contains("| detail"), "{rendered}");
        assert!(
            rendered.contains("→ openai-codex/gpt-5.3-codex-max"),
            "{rendered}"
        );
        assert!(rendered.contains("(selected)"), "{rendered}");
        assert_eq!(lines[2].spans[2].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn picker_selection_wraps() {
        let mut picker = UiPicker::new(
            "select model",
            "enter confirm",
            vec![
                PickerItem {
                    label: "model-a".into(),
                    detail: None,
                    preview: None,
                    badge: None,
                    value: "model-a".into(),
                },
                PickerItem {
                    label: "model-b".into(),
                    detail: None,
                    preview: None,
                    badge: None,
                    value: "model-b".into(),
                },
            ],
            PickerAction::SelectModel,
        );

        picker.select_previous();
        assert_eq!(picker.selected_item().unwrap().value, "model-b");
        picker.select_next();
        assert_eq!(picker.selected_item().unwrap().value, "model-a");
    }

    #[test]
    fn favorite_save_failure_keeps_model_picker_open() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut app = test_app();
        app.info.services.config_repository =
            ConfigRepository::new(Some(config_dir.path().to_path_buf()));
        let selected_value = "openai/gpt-5.5";
        app.composer = ComposerMode::Picker(UiPicker::new(
            "select model",
            "ctrl-p pin/unpin",
            vec![PickerItem {
                label: selected_value.into(),
                detail: None,
                preview: None,
                badge: None,
                value: selected_value.into(),
            }],
            PickerAction::SelectModel,
        ));
        app.toggle_selected_model_favorite().unwrap();

        assert!(matches!(app.composer, ComposerMode::Picker(_)));
        assert_eq!(app.active_picker_selection().unwrap().1, selected_value);
        assert!(app.info.runtime.favorite_models.is_empty());
        assert_eq!(app.status, "config save failed");
        assert!(matches!(
            app.transcript.last(),
            Some(Entry::Error(message)) if message.starts_with("could not save pinned models: ")
        ));
    }

    #[test]
    fn web_search_config_restore_keeps_api_key_row_selected() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut app = test_app();
        app.info.services.config_repository =
            ConfigRepository::new(Some(config_dir.path().join("config.toml")));
        let config = app.info.services.config_repository.load().unwrap();
        let mut picker =
            config_picker::web_search_config_picker(&config, app.credential_store.as_ref());

        App::restore_picker_position(
            &mut picker,
            config_picker::WEB_SEARCH_EXA_KEY_VALUE,
            String::new(),
        );

        assert_eq!(
            picker.selected_item().unwrap().value,
            config_picker::WEB_SEARCH_EXA_KEY_VALUE
        );
    }

    #[test]
    fn esc_from_nested_web_search_config_returns_to_tools_category() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut app = test_app();
        app.info.services.config_repository =
            ConfigRepository::new(Some(config_dir.path().join("config.toml")));
        let config = app.info.services.config_repository.load().unwrap();
        let mut root = config_picker::config_picker(&app.info.runtime, &config);
        App::restore_picker_position(
            &mut root,
            config_picker::TOOLS_CATEGORY_VALUE,
            String::new(),
        );
        let mut parent = config_picker::category_picker(
            config_picker::TOOLS_CATEGORY_VALUE,
            &app.info.runtime,
            &config,
        )
        .unwrap()
        .with_parent(root);
        App::restore_picker_position(&mut parent, config_picker::WEB_SEARCH_VALUE, "web".into());
        app.composer = ComposerMode::Picker(parent);
        let child = config_picker::web_search_config_picker(&config, app.credential_store.as_ref());
        app.open_child_picker(child);

        app.handle_picker_escape(/*running*/ false).unwrap();

        let ComposerMode::Picker(picker) = &app.composer else {
            panic!("expected picker after nested config escape");
        };
        assert_eq!(
            picker.selected_item().unwrap().value,
            config_picker::WEB_SEARCH_VALUE
        );
        assert_eq!(picker.filter, "web");
        assert_eq!(app.status, picker.title);
    }

    #[test]
    fn esc_from_main_config_still_closes_picker() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut app = test_app();
        app.info.services.config_repository =
            ConfigRepository::new(Some(config_dir.path().join("config.toml")));
        let config = app.info.services.config_repository.load().unwrap();
        app.composer =
            ComposerMode::Picker(config_picker::config_picker(&app.info.runtime, &config));

        app.handle_picker_escape(/*running*/ false).unwrap();

        assert!(matches!(app.composer, ComposerMode::Input));
        assert_eq!(app.status, "ready");
    }

    #[test]
    fn input_history_recalls_previous_messages_and_restores_draft() {
        let mut app = test_app();
        app.push_input_history("first message");
        app.push_input_history("second message");
        app.input = "draft".into();
        app.input_cursor = app.input_char_len();

        app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
        assert_eq!(app.input, "second message");
        assert_eq!(app.input_cursor, "second message".chars().count());

        app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
        assert_eq!(app.input, "first message");

        app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
        assert_eq!(app.input, "second message");

        app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
        assert_eq!(app.input, "draft");
        assert_eq!(app.input_history_cursor, None);
    }

    #[test]
    fn input_history_clears_paste_segments_and_restores_draft_segments() {
        let mut app = test_app();
        app.push_input_history("previous message long enough for marker");
        app.insert_pasted_input_text("alpha\nbeta");

        app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
        assert_eq!(app.input, "previous message long enough for marker");
        assert!(app.paste_segments.is_empty());
        assert_eq!(
            app.expanded_input(),
            "previous message long enough for marker"
        );

        app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
        assert_eq!(app.input, "[ pasted: 2 lines ]");
        assert_eq!(app.expanded_input(), "alpha\nbeta");
    }

    #[test]
    fn editing_input_exits_history_navigation() {
        let mut app = test_app();
        app.push_input_history("previous");
        app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);

        app.insert_input_char('!');

        assert_eq!(app.input, "previous!");
        assert_eq!(app.input_history_cursor, None);
        assert_eq!(app.input_history_draft, None);
    }

    #[test]
    fn command_selection_clamps_to_available_matches() {
        let mut app = test_app();
        app.input = "/".into();
        app.input_cursor = 1;
        app.clamp_command_selection();
        app.command_selection = 99;
        app.clamp_command_selection();
        assert_eq!(app.command_selection, app.command_matches().len() - 1);

        app.input = "/mo".into();
        app.input_cursor = 3;
        app.clamp_command_selection();
        assert_eq!(app.command_selection, 0);
    }

    #[test]
    fn command_suggestions_truncate_long_descriptions() {
        let project = tempfile::tempdir().unwrap();
        let skill_dir = project
            .path()
            .join(".agents/skills/zz-deterministic-truncation-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: zz-deterministic-truncation-skill\ndescription: this description is intentionally long enough to require truncation in a narrow command suggestion row\n---\nbody\n",
        )
        .unwrap();
        let mut app = test_app();
        app.info.runtime.cwd = project.path().to_path_buf();
        app.input = "/zz".into();
        app.input_cursor = 3;
        app.clamp_command_selection();

        let lines = app.command_suggestion_lines(40);

        assert!(lines.iter().any(|line| line_text(line).contains('…')));
        assert!(lines
            .iter()
            .all(|line| line_text(line).chars().count() <= 40));
    }

    #[test]
    fn slash_command_args_preserves_text_after_skill_command() {
        assert_eq!(
            slash_command_args("/skill:rust-review check this diff"),
            "check this diff"
        );
    }

    #[test]
    fn complete_slash_command_inserts_prefixed_skill_command() {
        let (input, cursor) = complete_slash_command("/cav", 4, "skill:caveman");

        assert_eq!(input, "/skill:caveman");
        assert_eq!(cursor, 14);
    }

    #[test]
    fn history_lines_include_header_transcript_pending_preview_but_not_activity_row() {
        let mut app = test_app();
        app.push_transcript_entry(Entry::User("hello".into()));
        app.tool_calls
            .preview(0, None, vec!["bash".into(), "cargo test".into()]);
        app.live_stream_preview = Some(LiveStreamPreview {
            kind: StreamKind::Assistant,
            text: "partial answer".into(),
            include_leading_blank: true,
        });
        app.running = true;
        app.loading_spinner.start();

        let rendered = app
            .history_lines(60, Instant::now())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("rho  v"), "{rendered}");
        assert!(rendered.contains("hello"), "{rendered}");
        assert!(rendered.contains("bash"), "{rendered}");
        assert!(rendered.contains("partial answer"), "{rendered}");
        assert!(!rendered.contains("starting"), "{rendered}");
    }

    #[test]
    fn exit_summary_is_minimal_and_session_only() {
        let mut app = test_app();
        assert_eq!(app.exit_summary(), None);

        app.info.session.session_id = Some("session-123".into());
        assert_eq!(
            app.exit_summary().as_deref(),
            Some("rho session saved: session-123")
        );
    }

    #[test]
    fn status_notice_suppresses_consecutive_duplicates() {
        let mut app = test_app();
        app.notify_status("input cleared; press ctrl-c again to quit");
        app.notify_status("input cleared; press ctrl-c again to quit");

        assert_eq!(
            app.transcript
                .iter()
                .filter(|entry| matches!(entry, Entry::Notice(text) if text == "input cleared; press ctrl-c again to quit"))
                .count(),
            1
        );
    }

    #[test]
    fn image_paste_is_unavailable_while_running() {
        let mut app = test_app();
        app.running = true;

        app.paste_clipboard_image();

        assert!(app.pending_images.is_empty());
        assert_eq!(
            app.status,
            "image paste is unavailable while a model turn is running"
        );
    }

    #[test]
    fn paste_normalization_converts_crlf_and_cr() {
        assert_eq!(normalize_paste("a\r\nb\rc"), "a\nb\nc");
    }
}
