use std::{
    collections::VecDeque,
    future::Future,
    io::Write,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use futures_util::{task::noop_waker_ref, FutureExt};
use history_cache::{CachedCodeBlock, HistoryLineCache};
use tokio::sync::{mpsc, oneshot};

use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
};
use ratatui::{
    backend::Backend,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    DefaultTerminal, Frame, Terminal,
};
mod activity;
mod command_palette;
mod config_editor;
mod config_picker;
mod copy_interaction;
mod doctor;
mod event_batch;
mod file_palette;
mod file_picker;
mod goal;
mod goal_command;
mod history_cache;
mod inline_shell;
mod keybindings;
mod limits_command;
mod local_commands;
mod local_diff;
mod login;
mod markdown;
mod message_history;
mod model_picker;
mod mouse;
mod mouse_capture;
mod paste_burst;
mod picker;
mod provider_picker;
mod questionnaire;
mod questionnaire_input;
mod render;
mod run_lifecycle;
mod scrollbar;
mod session_picker;
mod skill_picker;
mod statusline;
mod stream;
mod text_selection;
mod theme;
mod tool_diff;
mod turn_prompt;

use activity::LoadingSpinner;
use config_editor::{
    config_number_input_lines, config_text_input_lines, resolve_web_search_editor_value,
    ConfigMutation, ConfigNumberInput, ConfigNumberKey, ConfigNumberSave, ConfigTextInput,
    ConfigTextKey, ConfigToggle,
};
use copy_interaction::CodeBlockCopyTarget;
use goal::GoalState;
use inline_shell::InlineShellMode;
use markdown::{push_wrapped_markdown_without_copy_button, update_code_block_state};
use paste_burst::{PasteBurst, PasteBurstEnter};
use picker::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, UiPicker};
use questionnaire::{
    questionnaire_cursor_position, questionnaire_lines, questionnaire_notice_text,
    QuestionAnswerRequest, QuestionnaireCancelReason, QuestionnaireComposer, QuestionnaireReply,
    QuestionnaireResponseChannel,
};
use render::{
    char_prefix_display_width, display_width, entry_lines, input_cursor_index_on_visual_line,
    input_cursor_position, input_visual_lines, picker_lines, push_wrapped_text,
    session_header_lines, styled_line, tool_entry_lines, truncate_one_line, LineFill,
};
use scrollbar::{scroll_state_for_top_line, HistoryScrollbar, HistoryScrollbarDrag};
use statusline::{GoalStatus, StatusLine};
use stream::{AppendOnlyStream, StreamFragment};
use text_selection::{
    highlight_selection, render_copy_notice, ClipboardWriter, CopyNotice, TerminalClipboard,
    TextSelection,
};
use theme::Theme;
use turn_prompt::TurnPrompt;

use crate::{
    agent::{Agent, AgentEvent, ModelAndDisplayContent, QuestionnaireRequest, SessionHistorySink},
    app::config_repository::ConfigRepository,
    auth::{codex_oauth, github_copilot_device, xai_oauth},
    clipboard_image::read_clipboard_image,
    commands::{self, CommandId, CommandInvocation, CommandSpec},
    credentials::{
        available_auth_modes, delete_provider_credentials, load_web_search_api_key,
        provider_has_credentials, provider_has_env_override, save_codex_tokens,
        save_github_copilot_tokens, save_provider_api_key, save_xai_tokens, CodexTokens,
        CredentialStore, GitHubCopilotTokens, OsCredentialStore, XaiTokens,
    },
    herdr::{HerdrReporter, HerdrState},
    keybindings::Keybindings,
    model::{
        build_provider,
        catalog::{self, LoginTarget, ModelSelection},
        favorites, image_summary,
        models_dev::{cached_model_metadata, fetch_model_metadata},
        provider_models::refresh_provider_models_with_store,
        ContentBlock, ContextUsage, ImageContent, Message, ModelMetadata, ModelRequest,
        ModelResponse, ModelUsage, UnavailableProvider,
    },
    provider::{self, ProviderAuthKind},
    reasoning::ReasoningLevel,
    session::Session,
    tool::ToolDisplayStyle,
};
const DEFAULT_TUI_HEIGHT: u16 = 18;
const PASTE_COLLAPSE_MIN_LINES: usize = 2;
const PASTE_COLLAPSE_MIN_CHARS: usize = 1000;
const MAX_COMMAND_SUGGESTIONS: usize = 5;
const MAX_TERMINAL_EVENTS_PER_TICK: usize = 4096;
const RECOVERED_HISTORY_LINE_LIMIT: usize = 200;
const STREAM_PREVIEW_DELAY: Duration = Duration::from_millis(24);
const STREAM_PREVIEW_MIN_CHARS: usize = 2;
const HISTORY_SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(1200);
pub struct TuiInfo {
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
    pub reasoning: ReasoningLevel,
    pub show_reasoning_output: bool,
    pub auth: String,
    pub title_provider: Option<String>,
    pub title_model: Option<String>,
    pub title_auth: Option<String>,
    pub favorite_models: Vec<String>,
    pub max_tool_output_lines: usize,
    pub keybindings: Keybindings,
    pub prompt_templates: crate::prompt_templates::PromptTemplates,
    pub questionnaire_enabled: bool,
    pub session_id: Option<String>,
    pub recovered_messages: Vec<Message>,
    pub open_resume_picker: bool,
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
pub async fn run(agent: &mut Agent, info: TuiInfo) -> anyhow::Result<TuiResult> {
    agent.set_session_id(info.session_id.clone());
    let mut terminal = ratatui::init();
    Theme::initialize_from_terminal();
    let bracketed_paste_enabled = enable_bracketed_paste().is_ok();
    let mouse_capture_enabled = mouse_capture::enable().is_ok();
    let modified_keys_enabled = enable_modified_keys().is_ok();
    let keyboard_enhancements_enabled = enable_keyboard_enhancements().is_ok();
    let herdr = info.herdr.clone();
    let initial_state = if info.auth_unavailable.is_some() {
        HerdrState::Blocked
    } else {
        HerdrState::Idle
    };
    herdr
        .report_state(
            initial_state,
            info.auth_unavailable.as_deref(),
            info.session_id.as_deref(),
        )
        .await;
    let result = App::new(info).run(&mut terminal, agent).await;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenLayout {
    history: Rect,
    history_scrollbar: Option<HistoryScrollbar>,
    activity: Option<Rect>,
    jump_to_bottom: Option<Rect>,
    top_divider: Rect,
    composer: Rect,
    bottom_divider: Rect,
    statusline: Rect,
    commands: Rect,
    composer_start: usize,
    history_len: usize,
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

struct App {
    info: TuiInfo,
    statusline: StatusLine,
    input: String,
    input_cursor: usize,
    status: String,
    should_quit: bool,
    ctrl_c_streak: u8,
    assistant_stream: AppendOnlyStream,
    assistant_stream_in_code_block: bool,
    reasoning_stream: AppendOnlyStream,
    current_stream_kind: Option<StreamKind>,
    stream_preview_deadline: Option<Instant>,
    live_stream_preview: Option<LiveStreamPreview>,
    current_turn_start: Option<usize>,
    active_turn_show_reasoning_output: bool,
    hidden_reasoning_active: bool,
    running: bool,
    loading_spinner: LoadingSpinner,
    active_tool_call: bool,
    pending_tool_call: Option<ToolEntry>,
    steering_prompts: VecDeque<String>,
    queued_prompts: VecDeque<QueuedPrompt>,
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
    composer: ComposerMode,
    credential_store: Arc<dyn CredentialStore>,
    available_auths: Vec<String>,
    using_unavailable_provider: bool,
    pending_oauth_login: Option<PendingOAuthLogin>,
    cumulative_usage: Option<ModelUsage>,
    latest_usage: Option<ModelUsage>,
    current_context: Option<ContextUsage>,
    model_metadata: Option<ModelMetadata>,
    pending_model_metadata: Option<tokio::task::JoinHandle<Option<ModelMetadata>>>,
    pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pending_model_selection: Option<ModelSelection>,
    pending_session_title: Option<Pin<Box<dyn Future<Output = SessionTitleResult>>>>,
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
}

#[derive(Clone, Debug)]
struct SecretInput {
    target: LoginTarget,
    value: String,
    cursor: usize,
}

#[derive(Debug)]
struct PendingOAuthLogin {
    target: LoginTarget,
    handle: tokio::task::JoinHandle<Result<PendingOAuthResult, String>>,
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
enum PendingOAuthResult {
    Codex(CodexTokens),
    GithubCopilot(GitHubCopilotTokens),
    Xai(XaiTokens),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TurnOutcome {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryScroll {
    Bottom,
    Manual { top_line: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CommandChoiceKind {
    Builtin(&'static CommandSpec),
    PromptTemplate(String),
    Skill,
}

#[derive(Clone, Debug)]
struct ToolEntry {
    state: ToolEntryState,
    display_lines: Vec<String>,
    expanded: bool,
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
    #[allow(dead_code)]
    Assistant(String),
    Reasoning(String),
    Tool(ToolEntry),
    Notice(String),
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

impl SecretInput {
    fn new(target: LoginTarget) -> Self {
        Self {
            target,
            value: String::new(),
            cursor: 0,
        }
    }

    fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    fn insert_char(&mut self, ch: char) {
        let byte_index = self.byte_index(self.cursor);
        self.value.insert(byte_index, ch);
        self.cursor += 1;
    }

    fn insert_text(&mut self, text: &str) {
        let sanitized = text.replace('\n', "");
        let byte_index = self.byte_index(self.cursor);
        self.value.insert_str(byte_index, &sanitized);
        self.cursor += sanitized.chars().count();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }
}

impl App {
    fn new(info: TuiInfo) -> Self {
        Self::new_with_credentials(info, Arc::new(OsCredentialStore))
    }

    fn new_with_credentials(info: TuiInfo, credential_store: Arc<dyn CredentialStore>) -> Self {
        let available_auths = available_auth_modes(credential_store.as_ref());
        let using_unavailable_provider = info.auth_unavailable.is_some();
        let mut info = info;
        info.max_tool_output_lines = info.max_tool_output_lines.max(1);
        let status = info
            .auth_unavailable
            .as_ref()
            .map(|_| "no providers configured; run /login to sign in".into())
            .unwrap_or_else(|| "ready".into());
        let active_turn_show_reasoning_output = info.show_reasoning_output;
        let pending_update_notice = info.pending_update_notice.take();
        let statusline = StatusLine::new(&info);
        Self {
            info,
            statusline,
            input: String::new(),
            input_cursor: 0,
            status,
            should_quit: false,
            ctrl_c_streak: 0,
            assistant_stream: AppendOnlyStream::default(),
            assistant_stream_in_code_block: false,
            reasoning_stream: AppendOnlyStream::default(),
            current_stream_kind: None,
            stream_preview_deadline: None,
            live_stream_preview: None,
            current_turn_start: None,
            active_turn_show_reasoning_output,
            hidden_reasoning_active: false,
            running: false,
            loading_spinner: LoadingSpinner::default(),
            active_tool_call: false,
            pending_tool_call: None,
            steering_prompts: VecDeque::new(),
            queued_prompts: VecDeque::new(),
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
            composer: ComposerMode::Input,
            credential_store,
            available_auths,
            using_unavailable_provider,
            pending_oauth_login: None,
            cumulative_usage: None,
            latest_usage: None,
            current_context: None,
            model_metadata: None,
            pending_model_metadata: None,
            pending_update_notice,
            pending_model_selection: None,
            pending_session_title: None,
            history_scroll: HistoryScroll::Bottom,
            history_scrollbar_drag: None,
            history_scrollbar_visible_until: None,
            history_scrollbar_hovered: false,
            hovered_code_block_copy: None,
            text_selection: None,
            copy_notice: None,
            clipboard: Box::new(TerminalClipboard),
            session_header_cache: None,
            last_mouse_position: None,
        }
    }

    async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<TuiResult> {
        self.start_model_metadata_fetch(agent);
        self.insert_session_intro(terminal)?;
        self.insert_recovered_history(terminal)?;
        if self.info.open_resume_picker {
            self.open_resume_picker()?;
        }
        if self.info.auth_unavailable.is_some() {
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
                    .is_some_and(|pending| pending.handle.is_finished());
            self.poll_model_metadata_fetch(agent);
            self.poll_update_notice();
            needs_redraw |= self.poll_pending_session_title()?;
            self.poll_pending_oauth_login(terminal, agent).await?;
            needs_redraw |= background_ready;
            if !event::poll(Duration::from_millis(0))? {
                needs_redraw |= self.flush_due_paste_burst();
            }
            if needs_redraw {
                terminal.draw(|frame| self.draw(frame))?;
                needs_redraw = false;
            }
            let idle_timeout = if self.pending_model_metadata.is_some()
                || self.pending_update_notice.is_some()
                || self.pending_session_title.is_some()
                || self.pending_oauth_login.is_some()
            {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(3600)
            };
            let redraw_on_timeout = self.animation_active(Instant::now());
            if event::poll(self.event_poll_timeout(idle_timeout))? {
                let mut event_count = 0;
                loop {
                    let event = event::read()?;
                    self.handle_terminal_event(event, terminal, agent).await?;
                    needs_redraw = true;
                    event_count += 1;
                    if self.should_quit
                        || event_count >= MAX_TERMINAL_EVENTS_PER_TICK
                        || !event::poll(Duration::from_millis(0))?
                    {
                        break;
                    }
                }
            } else {
                needs_redraw |= self.flush_due_paste_burst();
                needs_redraw |= redraw_on_timeout;
            }
        }
        Ok(TuiResult {
            resume_session_id: self.info.session_id.clone(),
            exit_summary: self.exit_summary(),
        })
    }

    async fn handle_terminal_event(
        &mut self,
        event: Event,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
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

    fn flush_due_paste_burst(&mut self) -> bool {
        if self.paste_burst.is_due(Instant::now()) {
            self.flush_pending_paste_burst();
            true
        } else {
            false
        }
    }

    fn flush_pending_paste_burst(&mut self) {
        let Some(text) = self.paste_burst.take_pending() else {
            return;
        };
        let text = normalize_paste(&text);
        self.insert_paste(&text);
    }

    fn handle_paste_burst_key(&mut self, key: KeyEvent) -> bool {
        self.handle_paste_burst_key_at(key, Instant::now())
    }

    fn handle_paste_burst_key_at(&mut self, key: KeyEvent, now: Instant) -> bool {
        let Some(burst_key) = self.paste_burst_key(key) else {
            self.flush_pending_paste_burst();
            return false;
        };

        match burst_key {
            PasteBurstKey::Char(ch) => {
                if !self.paste_burst.can_continue(now) {
                    self.flush_pending_paste_burst();
                }
                self.paste_burst.push_plain_char(ch, now);
                self.ctrl_c_streak = 0;
                true
            }
            PasteBurstKey::Enter => match self.paste_burst.push_enter_if_paste(now) {
                PasteBurstEnter::Buffered => {
                    self.ctrl_c_streak = 0;
                    true
                }
                PasteBurstEnter::InsertNewline => {
                    self.insert_paste_burst_newline();
                    self.ctrl_c_streak = 0;
                    true
                }
                PasteBurstEnter::NotPaste => {
                    self.flush_pending_paste_burst();
                    false
                }
            },
        }
    }

    fn insert_paste_burst_newline(&mut self) {
        match &mut self.composer {
            ComposerMode::Input => self.insert_input_char('\n'),
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.insert_char('\n');
            }
            ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::OAuthPending(_) => {}
        }
    }

    fn paste_burst_key(&self, key: KeyEvent) -> Option<PasteBurstKey> {
        match (key.modifiers, key.code) {
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.composer_accepts_paste_burst_char(ch) =>
            {
                Some(PasteBurstKey::Char(ch))
            }
            (KeyModifiers::NONE, KeyCode::Enter) if self.composer_accepts_paste_burst_enter() => {
                Some(PasteBurstKey::Enter)
            }
            _ => None,
        }
    }

    fn composer_accepts_paste_burst_char(&self, ch: char) -> bool {
        match &self.composer {
            ComposerMode::Input => true,
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.accepts_paste_burst_char(ch)
            }
            ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::OAuthPending(_) => false,
        }
    }

    fn composer_accepts_paste_burst_enter(&self) -> bool {
        match &self.composer {
            ComposerMode::Input => true,
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.active_text_entry_active()
                    || (self.paste_burst.has_pending()
                        && questionnaire.accepts_pending_paste_burst_enter())
            }
            ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::OAuthPending(_) => false,
        }
    }

    async fn handle_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        if self.handle_paste_burst_key(key) {
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
                self.input_cursor = self.input_cursor.saturating_sub(1);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Right) => {
                self.input_cursor = (self.input_cursor + 1).min(self.input_char_len());
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Up) => {
                if self.recall_last_queued_prompt() {
                    self.notify_status(format!(
                        "editing queued message; {} queued message(s) remain",
                        self.queued_prompts.len()
                    ));
                }
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
            self.info.update_notice = Some(notice);
        }
    }

    fn start_model_metadata_fetch(&mut self, agent: &mut Agent) {
        if let Some(handle) = self.pending_model_metadata.take() {
            handle.abort();
        }
        if let Some(metadata) = cached_model_metadata(&self.info.provider, &self.info.model) {
            agent.set_context_window(metadata.display_context_window());
            self.model_metadata = Some(metadata);
            return;
        }

        agent.set_context_window(None);
        self.model_metadata = None;
        let provider = self.info.provider.clone();
        let model = self.info.model.clone();
        self.pending_model_metadata = Some(tokio::spawn(async move {
            fetch_model_metadata(&provider, &model).await
        }));
    }

    fn poll_model_metadata_fetch(&mut self, agent: &mut Agent) {
        let Some(handle) = self.pending_model_metadata.as_mut() else {
            return;
        };
        if !handle.is_finished() {
            return;
        }
        if let Some(handle) = self.pending_model_metadata.take() {
            if let Some(Ok(Some(metadata))) = handle.now_or_never() {
                agent.set_context_window(metadata.display_context_window());
                let reasoning = self
                    .info
                    .reasoning
                    .normalize(metadata.supported_reasoning_levels.as_deref());
                let provider_updated = if agent.set_provider_reasoning(reasoning) {
                    true
                } else {
                    match build_provider(&self.info.provider, &self.info.model, reasoning) {
                        Ok(provider) => {
                            agent.replace_provider(provider);
                            true
                        }
                        Err(err) => {
                            self.insert_entry(&Entry::Error(format!(
                                "could not apply model reasoning metadata: {err}"
                            )));
                            false
                        }
                    }
                };
                if provider_updated && reasoning != self.info.reasoning {
                    self.info.reasoning = reasoning;
                    self.info.diagnostics.update_identity(
                        &self.info.provider,
                        &self.info.model,
                        reasoning,
                    );
                    if let Err(err) = self.info.config_repository.update(|config| {
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
        let std::task::Poll::Ready(result) = future.as_mut().poll(&mut context) else {
            return Ok(false);
        };
        self.pending_session_title = None;
        let Ok(title) = result.title else {
            return Ok(false);
        };
        if Session::set_title(&self.info.cwd, &result.session_id, &title).is_err() {
            return Ok(false);
        }
        if self.info.session_id.as_deref() == Some(result.session_id.as_str()) {
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
        agent: &mut Agent,
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
                let saved = match input.save(&self.info.config_repository) {
                    Ok(saved) => saved,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "config save failed".into();
                        return Ok(true);
                    }
                };
                match saved {
                    ConfigNumberSave::MaxOutputBytes(value) => {
                        let config = self.info.config_repository.load()?;
                        self.composer =
                            ComposerMode::Picker(config_picker::config_picker(&self.info, &config));
                        self.insert_entry(&Entry::Notice(format!(
                            "max output bytes set to {value}; applies next session"
                        )));
                    }
                    ConfigNumberSave::MaxToolOutputLines(value) => {
                        self.info.max_tool_output_lines = value;
                        self.info.diagnostics.update_max_tool_output_lines(value);
                        let config = self.info.config_repository.load()?;
                        self.composer =
                            ComposerMode::Picker(config_picker::config_picker(&self.info, &config));
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
                let config = self.info.config_repository.load()?;
                self.info.show_reasoning_output = config.show_reasoning_output;
                self.composer =
                    ComposerMode::Picker(config_picker::config_picker(&self.info, &config));
                self.status = "config".into();
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
        agent: &mut Agent,
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
        agent: &mut Agent,
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
        agent: &mut Agent,
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

    fn input_char_len(&self) -> usize {
        self.input.chars().count()
    }

    fn input_byte_index(&self, char_index: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.input.len())
    }

    fn reset_input_history_navigation(&mut self) {
        self.input_history_cursor = None;
        self.input_history_draft = None;
    }

    fn push_input_history(&mut self, prompt: &str) {
        if prompt.is_empty() || self.input_history.last().is_some_and(|last| last == prompt) {
            return;
        }
        self.input_history.push(prompt.to_string());
    }

    fn recall_input_history(&mut self, direction: HistoryDirection) -> bool {
        if self.input_history.is_empty() {
            return false;
        }

        let next_cursor = match (direction, self.input_history_cursor) {
            (HistoryDirection::Previous, None) => {
                self.input_history_draft = Some(InputDraft {
                    input: self.input.clone(),
                    paste_segments: self.paste_segments.clone(),
                    submission_mode: self.input_submission_mode,
                });
                self.input_history.len() - 1
            }
            (HistoryDirection::Previous, Some(0)) => 0,
            (HistoryDirection::Previous, Some(cursor)) => cursor - 1,
            (HistoryDirection::Next, None) => return false,
            (HistoryDirection::Next, Some(cursor)) if cursor + 1 < self.input_history.len() => {
                cursor + 1
            }
            (HistoryDirection::Next, Some(_)) => {
                let draft = self.input_history_draft.take().unwrap_or(InputDraft {
                    input: String::new(),
                    paste_segments: Vec::new(),
                    submission_mode: InputSubmissionMode::ParseCommands,
                });
                self.input = draft.input;
                self.paste_segments = draft.paste_segments;
                self.input_submission_mode = draft.submission_mode;
                self.input_cursor = self.input_char_len();
                self.input_history_cursor = None;
                self.input_changed();
                return true;
            }
        };

        self.input = self.input_history[next_cursor].clone();
        self.paste_segments.clear();
        self.input_submission_mode = InputSubmissionMode::ParseCommands;
        self.input_cursor = self.input_char_len();
        self.input_history_cursor = Some(next_cursor);
        self.input_changed();
        true
    }

    fn recall_input_history_or_move_cursor(
        &mut self,
        direction: HistoryDirection,
        terminal_width: usize,
    ) {
        let visual_lines = input_visual_lines(&self.input, terminal_width);
        let cursor_position = input_cursor_position(&self.input, self.input_cursor, terminal_width);
        let can_recall = match direction {
            HistoryDirection::Previous => cursor_position.y == 0,
            HistoryDirection::Next => cursor_position.y as usize + 1 >= visual_lines.len(),
        };

        if can_recall && self.recall_input_history(direction) {
            return;
        }

        let target_row = match direction {
            HistoryDirection::Previous => cursor_position.y.saturating_sub(1) as usize,
            HistoryDirection::Next => cursor_position.y as usize + 1,
        };
        self.input_cursor = input_cursor_index_on_visual_line(
            &self.input,
            &visual_lines,
            target_row,
            cursor_position.x as usize,
        );
    }

    fn recall_last_queued_prompt(&mut self) -> bool {
        let Some(prompt) = self.queued_prompts.pop_back() else {
            return false;
        };
        self.input = prompt.display_prompt;
        self.paste_segments = prompt.paste_segments;
        self.input_submission_mode = InputSubmissionMode::ParseCommands;
        self.input_cursor = self.input_char_len();
        self.reset_input_history_navigation();
        self.input_changed();
        true
    }

    fn replace_input_range(&mut self, start: usize, end: usize, text: &str) {
        self.reset_input_history_navigation();
        self.adjust_paste_segments_for_edit(start, end.saturating_sub(start), text.chars().count());
        let start_byte = self.input_byte_index(start);
        let end_byte = self.input_byte_index(end);
        self.input.replace_range(start_byte..end_byte, text);
        self.input_cursor = start + text.chars().count();
        self.input_changed();
    }

    fn insert_input_char(&mut self, ch: char) {
        self.reset_input_history_navigation();
        self.adjust_paste_segments_for_edit(self.input_cursor, 0, 1);
        let byte_index = self.input_byte_index(self.input_cursor);
        self.input.insert(byte_index, ch);
        self.input_cursor += 1;
        self.input_changed();
    }

    fn insert_input_text(&mut self, text: &str) {
        self.insert_input_text_with_paste_content(text, None);
    }

    fn insert_pasted_input_text(&mut self, text: &str) {
        let Some(marker) = paste_marker_for(text) else {
            self.insert_input_text(text);
            return;
        };
        self.insert_input_text_with_paste_content(&marker, Some(text.to_string()));
    }

    fn insert_input_text_with_paste_content(&mut self, text: &str, paste_content: Option<String>) {
        self.reset_input_history_navigation();
        let start = self.input_cursor;
        let inserted_len = text.chars().count();
        self.adjust_paste_segments_for_edit(start, 0, inserted_len);
        let byte_index = self.input_byte_index(start);
        self.input.insert_str(byte_index, text);
        self.input_cursor += inserted_len;
        if let Some(content) = paste_content {
            self.paste_segments.push(PasteSegment {
                start,
                marker_len: inserted_len,
                content,
            });
            self.paste_segments.sort_by_key(|segment| segment.start);
        }
        self.input_changed();
    }

    fn expanded_input(&self) -> String {
        expand_paste_segments(&self.input, &self.paste_segments)
    }

    fn adjust_paste_segments_for_edit(
        &mut self,
        start: usize,
        deleted_len: usize,
        inserted_len: usize,
    ) {
        let end = start + deleted_len;
        let shift = inserted_len as isize - deleted_len as isize;
        self.paste_segments.retain_mut(|segment| {
            if start < segment.end() && end > segment.start {
                return false;
            }
            if start <= segment.start {
                segment.start = segment.start.saturating_add_signed(shift);
            }
            true
        });
    }

    fn backspace_input(&mut self) {
        if self.input_cursor == 0 {
            if self.input.is_empty() && self.pending_images.pop().is_some() {
                self.status = format!("attached images: {}", self.pending_images.len());
            }
            return;
        }
        self.reset_input_history_navigation();
        let edit_start = self.input_cursor - 1;
        self.adjust_paste_segments_for_edit(edit_start, 1, 0);
        let start = self.input_byte_index(edit_start);
        let end = self.input_byte_index(self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor -= 1;
        self.input_changed();
    }

    fn delete_input(&mut self) {
        if self.input_cursor >= self.input_char_len() {
            return;
        }
        self.reset_input_history_navigation();
        self.adjust_paste_segments_for_edit(self.input_cursor, 1, 0);
        let start = self.input_byte_index(self.input_cursor);
        let end = self.input_byte_index(self.input_cursor + 1);
        self.input.replace_range(start..end, "");
        self.input_changed();
    }

    fn delete_word_before_cursor(&mut self) {
        self.reset_input_history_navigation();
        let start_cursor = previous_word_boundary(&self.input, self.input_cursor);
        self.adjust_paste_segments_for_edit(start_cursor, self.input_cursor - start_cursor, 0);
        let start = self.input_byte_index(start_cursor);
        let end = self.input_byte_index(self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor = start_cursor;
        self.input_changed();
    }

    fn input_changed(&mut self) {
        self.command_palette_dismissed = false;
        self.file_palette_dismissed = false;
        self.clamp_command_selection();
        self.clamp_file_selection();
    }

    fn parse_input_command(
        &mut self,
    ) -> Result<Option<CommandInvocation>, commands::CommandParseError> {
        match std::mem::take(&mut self.input_submission_mode) {
            InputSubmissionMode::ParseCommands => commands::parse_command(&self.input),
            InputSubmissionMode::Prompt => Ok(None),
        }
    }

    fn command_palette_visible(&self) -> bool {
        matches!(self.composer, ComposerMode::Input)
            && !self.command_palette_dismissed
            && self.cursor_in_command_token()
            && !self.command_matches().is_empty()
    }

    fn cursor_in_command_token(&self) -> bool {
        if !self.input.starts_with('/') {
            return false;
        }

        let token_len = self
            .input
            .chars()
            .position(char::is_whitespace)
            .unwrap_or_else(|| self.input_char_len());
        self.input_cursor <= token_len
    }

    fn clamp_command_selection(&mut self) {
        let prefix = self
            .cursor_in_command_token()
            .then(|| commands::command_prefix(&self.input).map(str::to_ascii_lowercase))
            .flatten();
        if self.command_prefix != prefix {
            self.command_prefix = prefix;
            self.command_selection = 0;
        }

        let match_count = self.command_matches().len();
        if match_count == 0 {
            self.command_selection = 0;
        } else if self.command_selection >= match_count {
            self.command_selection = match_count - 1;
        }
    }

    fn ensure_session(&mut self, agent: &mut Agent) -> anyhow::Result<()> {
        if self.info.session_id.is_none() {
            let session = Session::create(&self.info.cwd)?;
            let session_id = session.id().to_string();
            self.info.session_id = Some(session_id.clone());
            agent.set_session_id(Some(session_id));
            agent.set_history_sink(SessionHistorySink::new(session));
        }
        Ok(())
    }

    fn title_model_selection(&self) -> (String, String, String) {
        (
            self.info
                .title_provider
                .clone()
                .unwrap_or_else(|| self.info.provider.clone()),
            self.info
                .title_model
                .clone()
                .unwrap_or_else(|| self.info.model.clone()),
            self.info
                .title_auth
                .clone()
                .unwrap_or_else(|| self.info.auth.clone()),
        )
    }

    fn start_session_title_generation(&mut self, first_user_message: String) {
        let Some(session_id) = self.info.session_id.clone() else {
            return;
        };
        self.pending_session_title = None;
        let (provider, model, _auth) = self.title_model_selection();
        self.pending_session_title = Some(Box::pin(async move {
            let title = generate_session_title(provider, model, first_user_message).await;
            SessionTitleResult { session_id, title }
        }));
    }

    async fn submit(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let mut prompt = self.expanded_input().trim().to_string();
        let mut display_prompt = self.input.trim().to_string();
        if prompt.is_empty() && self.pending_images.is_empty() {
            self.clear_submitted_input();
            return Ok(());
        }
        if let Some((mode, command)) = InlineShellMode::parse(self.input.trim()) {
            if !self.paste_segments.is_empty() {
                return self.block_pasted_inline_shell();
            }
            let command = command.to_string();
            self.clear_submitted_input();
            self.execute_inline_shell(mode, command, terminal, agent)
                .await?;
            return Ok(());
        }

        match self.parse_input_command() {
            Ok(Some(mut invocation)) => {
                if invocation.id == CommandId::Goal {
                    invocation.raw_args = slash_command_args(&prompt).to_string();
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
                let expanded_input = self.expanded_input();
                let trailing_prompt = slash_command_args(&expanded_input).trim().to_string();
                let trailing_display_prompt = slash_command_args(&self.input).trim().to_string();
                self.input.clear();
                self.paste_segments.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                let template = name
                    .get(.."prompt:".len())
                    .filter(|prefix| prefix.eq_ignore_ascii_case("prompt:"))
                    .and_then(|_| name.get("prompt:".len()..))
                    .and_then(|template_name| {
                        crate::prompt_templates::find(&self.info.prompt_templates, template_name)
                    });
                if let Some(template) = template {
                    prompt = crate::prompt_templates::expand(template, &trailing_prompt);
                    display_prompt = prompt.clone();
                } else if self.execute_skill_command(&name, agent)? {
                    if trailing_prompt.is_empty() {
                        return Ok(());
                    }
                    prompt = trailing_prompt;
                    display_prompt = trailing_display_prompt;
                } else {
                    self.insert_entry(&Entry::Error(format!(
                        "unknown command '/{name}'. Type / to choose one of: {}",
                        commands::COMMANDS
                            .iter()
                            .map(|command| command.usage)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                    self.status = "unknown command".into();
                    return Ok(());
                }
            }
        }

        let images = std::mem::take(&mut self.pending_images);
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
        let mut outcome = self
            .run_prompt_turn(
                TurnPrompt::standard(prompt, display_prompt),
                images,
                terminal,
                agent,
            )
            .await?;
        while matches!(outcome, TurnOutcome::Completed) && !self.should_quit {
            let Some(prompt) = self.queued_prompts.pop_front() else {
                break;
            };
            outcome = self
                .run_prompt_turn(
                    TurnPrompt::standard(prompt.prompt, prompt.display_prompt),
                    Vec::new(),
                    terminal,
                    agent,
                )
                .await?;
        }
        if matches!(outcome, TurnOutcome::Completed) && self.goal.is_some() {
            self.continue_goal(terminal, agent).await?;
        }
        Ok(())
    }

    async fn run_prompt_turn(
        &mut self,
        prompt: TurnPrompt,
        images: Vec<ImageContent>,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<TurnOutcome> {
        if !prompt.history.is_empty() {
            self.push_input_history(&prompt.history);
        }
        self.reset_input_history_navigation();
        self.ensure_session(agent)?;
        self.info
            .herdr
            .report_session(self.info.session_id.as_deref())
            .await;
        if !agent
            .messages()
            .iter()
            .any(|message| matches!(message, Message::User(_)))
        {
            self.start_session_title_generation(prompt.history.clone());
        }
        self.insert_entry(&Entry::User(render_user_entry(&prompt.display, &images)));
        self.current_turn_start = Some(self.transcript.len());
        self.active_turn_show_reasoning_output = self.info.show_reasoning_output;
        self.reset_streams();
        self.hidden_reasoning_active = !self.active_turn_show_reasoning_output;
        self.status = "running".into();
        self.running = true;
        self.info
            .herdr
            .report_state(HerdrState::Working, None, self.info.session_id.as_deref())
            .await;
        self.loading_spinner.start();
        self.clamp_history_scroll_for_terminal(terminal)?;
        terminal.draw(|frame| self.draw(frame))?;

        if let Ok(config) = self.info.config_repository.load() {
            agent.set_compaction_config((&config).into());
        }
        self.active_tool_call = false;
        self.pending_tool_call = None;
        let interrupt_requested = Arc::new(AtomicBool::new(false));
        let cancellation = crate::cancellation::RunCancellation::default();
        let tool_call_active = Arc::new(AtomicBool::new(false));
        let steering_prompts = Arc::new(Mutex::new(VecDeque::new()));
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let result = {
            let callback_interrupt_requested = Arc::clone(&interrupt_requested);
            let run_interrupt_requested = Arc::clone(&interrupt_requested);
            let run_cancellation = cancellation.clone();
            let callback_tool_call_active = Arc::clone(&tool_call_active);
            let run_steering_prompts = Arc::clone(&steering_prompts);
            let (question_tx, mut question_rx) = mpsc::unbounded_channel::<QuestionAnswerRequest>();
            let mut content = Vec::with_capacity(1 + images.len());
            if !prompt.model.is_empty() {
                content.push(ContentBlock::Text(prompt.model));
            }
            let display_content = prompt.persisted_display.map(|display| {
                let mut display_content = Vec::with_capacity(1 + images.len());
                display_content.push(ContentBlock::Text(display));
                display_content.extend(images.iter().cloned().map(ContentBlock::Image));
                display_content
            });
            content.extend(images.into_iter().map(ContentBlock::Image));
            let user_content = match display_content {
                Some(display) => ModelAndDisplayContent::Separate {
                    model: content,
                    display,
                },
                None => ModelAndDisplayContent::Same(content),
            };
            let question_request_tx = question_tx.clone();
            let mut ask_questionnaire =
                move |request: QuestionnaireRequest| -> crate::agent::QuestionnaireFuture {
                    let question_request_tx = question_request_tx.clone();
                    let (reply_tx, reply_rx) = oneshot::channel();
                    Box::pin(async move {
                        question_request_tx
                            .send(QuestionAnswerRequest {
                                request,
                                response: QuestionnaireResponseChannel::new(reply_tx),
                            })
                            .map_err(|_| {
                                crate::agent::AgentError::Questionnaire(
                                    "questionnaire UI is unavailable".into(),
                                )
                            })?;
                        match reply_rx.await {
                            Ok(QuestionnaireReply::Answer(response)) => Ok(response),
                            Ok(QuestionnaireReply::Cancelled(
                                QuestionnaireCancelReason::UserCancelled,
                            )) => Err(crate::agent::AgentError::Questionnaire(
                                "questionnaire answer was cancelled".into(),
                            )),
                            Ok(QuestionnaireReply::Cancelled(
                                QuestionnaireCancelReason::UiUnavailable,
                            ))
                            | Err(_) => Err(crate::agent::AgentError::Questionnaire(
                                "questionnaire UI is unavailable".into(),
                            )),
                        }
                    })
                };
            let questionnaire_handler = self
                .info
                .questionnaire_enabled
                .then_some(&mut ask_questionnaire as crate::agent::QuestionnaireHandler<'_>);
            let mut run_future = Box::pin(
                agent.run_with_model_and_display_content_events_questionnaire_and_steering(
                    user_content,
                    move |event| {
                        match &event {
                            AgentEvent::ToolStarted { .. } => {
                                callback_tool_call_active.store(true, Ordering::SeqCst)
                            }
                            AgentEvent::ToolFinished { .. } => {
                                callback_tool_call_active.store(false, Ordering::SeqCst)
                            }
                            AgentEvent::StepStarted(_)
                            | AgentEvent::OutputDelta(_)
                            | AgentEvent::ReasoningDelta(_)
                            | AgentEvent::ContextUsage(_)
                            | AgentEvent::Usage(_)
                            | AgentEvent::ToolUpdated { .. }
                            | AgentEvent::ToolCallUpdated { .. }
                            | AgentEvent::QuestionnaireStarted(_)
                            | AgentEvent::QuestionnaireFinished(_) => {}
                        }
                        let _ = event_tx.send(event);
                        if callback_interrupt_requested.load(Ordering::SeqCst) {
                            return Err(crate::model::ModelError::Interrupted);
                        }
                        Ok(())
                    },
                    questionnaire_handler,
                    run_cancellation,
                    move || run_interrupt_requested.load(Ordering::SeqCst),
                    move || Ok(run_steering_prompts.lock().unwrap().pop_front()),
                ),
            );
            loop {
                tokio::select! {
                    result = &mut run_future => {
                        let mut result = result;
                        while let Ok(event) = event_rx.try_recv() {
                            if let Err(err) = self.handle_queued_agent_event(event, terminal) {
                                result = Err(crate::agent::AgentError::Provider(err));
                                break;
                            }
                        }
                        terminal.draw(|frame| self.draw(frame))?;
                        break result;
                    }
                    Some(request) = question_rx.recv() => {
                        self.open_questionnaire(request, terminal)?;
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                    Some(event) = event_rx.recv() => {
                        event_batch::handle_batch(event, &mut event_rx, |event| self.handle_queued_agent_event(event, terminal)).map_err(crate::agent::AgentError::Provider)?;
                        match self.handle_running_terminal_events(
                            terminal,
                            &interrupt_requested,
                            &tool_call_active,
                            RunningInputMode::Turn,
                        ) {
                            Ok(StreamControl::Interrupt) if !tool_call_active.load(Ordering::SeqCst) => {
                                cancellation.cancel();
                            }
                            Ok(StreamControl::Interrupt | StreamControl::Continue | StreamControl::Resize) => {}
                            Err(err) => break Err(crate::agent::AgentError::Provider(err)),
                        }
                        self.drain_steering_prompts_to(&steering_prompts);
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                    _ = tokio::time::sleep_until(self.stream_sleep_deadline()) => {
                        self.drain_stream_preview(terminal)?;
                        match self.handle_running_terminal_events(
                            terminal,
                            &interrupt_requested,
                            &tool_call_active,
                            RunningInputMode::Turn,
                        ) {
                            Ok(StreamControl::Interrupt) if !tool_call_active.load(Ordering::SeqCst) => {
                                cancellation.cancel();
                            }
                            Ok(StreamControl::Interrupt | StreamControl::Continue | StreamControl::Resize) => {}
                            Err(err) => break Err(crate::agent::AgentError::Provider(err)),
                        }
                        self.drain_steering_prompts_to(&steering_prompts);
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                }
            }
        };

        while let Ok(event) = event_rx.try_recv() {
            self.handle_agent_event(event, terminal)?;
        }
        self.active_tool_call = false;
        self.pending_tool_call = None;
        tool_call_active.store(false, Ordering::SeqCst);
        let outcome = match result {
            Ok(answer) => {
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams(terminal)?;
                self.insert_final_answer_suffix(terminal, &answer)?;
                self.reset_streams();
                self.current_turn_start = None;
                self.status = if self.queued_prompts.is_empty() {
                    "ready".into()
                } else {
                    format!(
                        "running next queued message ({})",
                        self.queued_prompts.len()
                    )
                };
                TurnOutcome::Completed
            }
            Err(crate::agent::AgentError::Provider(crate::model::ModelError::Interrupted)) => {
                self.restore_pending_work_to_input(&steering_prompts);
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams(terminal)?;
                self.insert_entry(&Entry::Notice("model interrupted".into()));
                self.reset_streams();
                self.current_turn_start = None;
                self.status = "interrupted".into();
                TurnOutcome::Interrupted
            }
            Err(err) => {
                self.reset_streams();
                self.current_turn_start = None;
                self.running = false;
                self.loading_spinner.stop();
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "error".into();
                TurnOutcome::Failed
            }
        };
        self.apply_pending_model_selection(agent)?;
        self.report_resting_herdr_state().await;
        terminal.draw(|frame| self.draw(frame))?;
        Ok(outcome)
    }

    async fn report_resting_herdr_state(&self) {
        let state = if self.info.auth_unavailable.is_some() {
            HerdrState::Blocked
        } else {
            HerdrState::Idle
        };
        self.info
            .herdr
            .report_state(
                state,
                self.info.auth_unavailable.as_deref(),
                self.info.session_id.as_deref(),
            )
            .await;
    }

    fn drain_steering_prompts_to(&mut self, target: &Arc<Mutex<VecDeque<String>>>) {
        if self.steering_prompts.is_empty() {
            return;
        }
        let mut target = target.lock().unwrap();
        target.extend(self.steering_prompts.drain(..));
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
            ComposerMode::Picker(_) | ComposerMode::OAuthPending(_) => {}
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

        if self.handle_history_key(key, terminal)? {
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
                self.input_cursor = self.input_cursor.saturating_sub(1);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Right) => {
                self.input_cursor = (self.input_cursor + 1).min(self.input_char_len());
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Up) => {
                if self.recall_last_queued_prompt() {
                    self.notify_status(format!(
                        "editing queued message; {} queued message(s) remain",
                        self.queued_prompts.len()
                    ));
                }
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
        if prompt.is_empty() {
            self.input.clear();
            self.paste_segments.clear();
            self.input_cursor = 0;
            self.clamp_command_selection();
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
                self.queue_steering_prompt(prompt)?;
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

    fn queue_steering_prompt(&mut self, prompt: String) -> anyhow::Result<()> {
        self.reset_input_history_navigation();
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
        self.steering_prompts.push_back(prompt);
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
                &self.info,
                self.pending_model_selection.as_ref(),
                &self.available_auths,
            );
            if picker.items.is_empty() {
                self.insert_entry(&Entry::Notice(
                    "no cached API models. run /refresh-model-list after the current run ends."
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
        match catalog::resolve_model_selection_for_auths(
            model,
            &self.info.provider,
            &self.info.auth,
            &self.available_auths,
        ) {
            Ok(selection) => self.queue_model_selection(selection),
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "model switch failed".into();
                Ok(())
            }
        }
    }

    fn queue_model_selection(&mut self, selection: ModelSelection) -> anyhow::Result<()> {
        let provider_model = format!("{}/{}", selection.provider, selection.model);
        self.pending_model_selection = Some(selection);
        self.insert_entry(&Entry::Notice(format!(
                "model change to {provider_model} queued; the current agent run will finish on its existing model, and the change will apply after the full run ends"
            )),
        );
        self.status = format!("model queued: {provider_model}");
        Ok(())
    }

    fn apply_pending_model_selection(&mut self, agent: &mut Agent) -> anyhow::Result<()> {
        let Some(selection) = self.pending_model_selection.take() else {
            return Ok(());
        };
        self.select_model(selection, agent)
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
            CommandId::Diff => self.execute_diff_command(),
            CommandId::Doctor => self.execute_doctor_command(),
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::TitleModel => self.execute_title_model_command(invocation, terminal),
            CommandId::Goal => self.execute_goal_command_during_turn(invocation),
            CommandId::Model => self.execute_model_command_during_turn(invocation),
            CommandId::New
            | CommandId::Compact
            | CommandId::Limits
            | CommandId::RefreshModelList
            | CommandId::Login
            | CommandId::Logout
            | CommandId::Resume => {
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
            PickerAction::Config => self.submit_config_selection_during_turn(&value)?,
            PickerAction::Doctor => {
                self.status = "running".into();
            }
            PickerAction::SelectTitleModel => {
                self.refresh_available_auths();
                let (provider, _model, auth) = self.title_model_selection();
                match catalog::resolve_model_selection_for_auths(
                    &value,
                    &provider,
                    &auth,
                    &self.available_auths,
                ) {
                    Ok(selection) => self.select_title_model(selection)?,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "title model switch failed".into();
                    }
                }
            }
            PickerAction::SelectModel => {
                self.refresh_available_auths();
                match catalog::resolve_model_selection_for_auths(
                    &value,
                    &self.info.provider,
                    &self.info.auth,
                    &self.available_auths,
                ) {
                    Ok(selection) => self.queue_model_selection(selection)?,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "model switch failed".into();
                    }
                }
            }
            PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::ResumeSession => {
                self.insert_entry(&Entry::Notice(
                    "that picker action is unavailable while a model turn is running".into(),
                ));
                self.status = "picker action unavailable while running".into();
            }
        }
        Ok(())
    }

    fn submit_config_selection_during_turn(&mut self, value: &str) -> anyhow::Result<()> {
        match value {
            config_picker::MAX_OUTPUT_BYTES_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxOutputBytes,
                    config.max_output_bytes,
                ));
                self.status = "edit max output bytes".into();
            }
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                let config = self.info.config_repository.load()?;
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
            config_picker::AUTO_COMPACT_VALUE => {
                self.toggle_auto_compact()?;
            }
            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactThresholdPercent,
                    config.compact_threshold_percent as usize,
                ));
                self.status = "edit compact threshold percent".into();
            }
            config_picker::COMPACT_TARGET_PERCENT_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactTargetPercent,
                    config.compact_target_percent as usize,
                ));
                self.status = "edit compact target percent".into();
            }
            config_picker::INLINE_SHELL_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::Picker(config_picker::inline_shell_picker(&config));
                self.status = "select inline shell".into();
            }
            value if value.starts_with(config_picker::INLINE_SHELL_PREFIX) => {
                let shell = value[config_picker::INLINE_SHELL_PREFIX.len()..].to_string();
                self.info.config_repository.update(|config| {
                    config.inline_shell.clone_from(&shell);
                })?;
                self.open_main_config_picker_selected(config_picker::INLINE_SHELL_VALUE)?;
                self.status = format!("inline shell: {shell}");
            }
            config_picker::WEB_SEARCH_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::Picker(config_picker::web_search_config_picker(
                    &config,
                    self.credential_store.as_ref(),
                ));
                self.status = "web search config".into();
            }
            config_picker::WEB_SEARCH_BACK_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer =
                    ComposerMode::Picker(config_picker::config_picker(&self.info, &config));
                self.status = "config".into();
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
        self.assistant_stream_in_code_block = false;
        self.reasoning_stream.reset();
        self.current_stream_kind = None;
        self.stream_preview_deadline = None;
        self.live_stream_preview = None;
        self.hidden_reasoning_active = false;
    }

    fn loading_active(&self) -> bool {
        self.running || !self.assistant_stream.is_empty() || !self.reasoning_stream.is_empty()
    }

    fn handle_queued_agent_event(
        &mut self,
        event: AgentEvent,
        terminal: &mut DefaultTerminal,
    ) -> Result<(), crate::model::ModelError> {
        self.handle_agent_event(event, terminal)?;
        Ok(())
    }

    fn stream_sleep_deadline(&self) -> tokio::time::Instant {
        let spinner_deadline = Instant::now() + LoadingSpinner::FRAME_INTERVAL;
        let deadline = self
            .stream_preview_deadline
            .map_or(spinner_deadline, |stream_deadline| {
                stream_deadline.min(spinner_deadline)
            });
        let deadline = self
            .paste_burst
            .deadline()
            .map_or(deadline, |paste_deadline| paste_deadline.min(deadline));
        tokio::time::Instant::from_std(deadline)
    }

    fn handle_running_terminal_events(
        &mut self,
        terminal: &mut DefaultTerminal,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
        input_mode: RunningInputMode,
    ) -> Result<StreamControl, crate::model::ModelError> {
        let mut control = StreamControl::Continue;
        while event::poll(Duration::from_millis(0))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    self.text_selection = None;
                    if key.code == KeyCode::Esc && !self.running_escape_has_overlay_target() {
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                    if input_mode == RunningInputMode::Turn {
                        self.handle_key_during_turn(key, terminal).map_err(|err| {
                            crate::model::ModelError::InvalidResponse(err.to_string())
                        })?;
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

    fn handle_agent_event(
        &mut self,
        event: AgentEvent,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<bool> {
        match event {
            AgentEvent::OutputDelta(text) => {
                self.hidden_reasoning_active = false;
                let switched = self.switch_stream_kind(terminal, StreamKind::Assistant)?;
                self.assistant_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Assistant)?;
                self.update_stream_preview_deadline(StreamKind::Assistant);
                Ok(switched || drained)
            }
            AgentEvent::ReasoningDelta(text) => {
                if !self.active_turn_show_reasoning_output {
                    self.hidden_reasoning_active = true;
                    return Ok(true);
                }
                let switched = self.switch_stream_kind(terminal, StreamKind::Reasoning)?;
                self.reasoning_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Reasoning)?;
                self.update_stream_preview_deadline(StreamKind::Reasoning);
                Ok(switched || drained)
            }
            other => {
                if matches!(
                    other,
                    AgentEvent::StepStarted(_)
                        | AgentEvent::ToolCallUpdated { .. }
                        | AgentEvent::ToolStarted { .. }
                        | AgentEvent::ToolFinished { .. }
                ) {
                    self.hidden_reasoning_active = false;
                    self.finish_streams(terminal)?;
                }
                if let Some(entry) = self.record_agent_event(other) {
                    self.insert_entry(&entry);
                }
                self.drain_streams(terminal)?;
                Ok(true)
            }
        }
    }

    fn switch_stream_kind(
        &mut self,
        terminal: &mut DefaultTerminal,
        kind: StreamKind,
    ) -> std::io::Result<bool> {
        let inserted = if self
            .current_stream_kind
            .is_some_and(|current| current != kind)
        {
            self.finish_current_stream(terminal)?
        } else {
            false
        };
        self.current_stream_kind = Some(kind);
        self.update_stream_preview_deadline(kind);
        Ok(inserted)
    }

    fn drain_streams(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<bool> {
        let reasoning_drained = self.drain_stream(terminal, StreamKind::Reasoning)?;
        let assistant_drained = self.drain_stream(terminal, StreamKind::Assistant)?;
        Ok(reasoning_drained || assistant_drained)
    }

    fn drain_stream(
        &mut self,
        terminal: &mut DefaultTerminal,
        kind: StreamKind,
    ) -> std::io::Result<bool> {
        let width = terminal.size()?.width as usize;
        let inner_width = padded_content_width(width);
        let fragment = match kind {
            StreamKind::Assistant => self
                .assistant_stream
                .drain_renderable_markdown(inner_width, self.assistant_stream_in_code_block),
            StreamKind::Reasoning => self.reasoning_stream.drain_renderable(inner_width),
        };
        if let Some(fragment) = fragment {
            self.live_stream_preview = None;
            self.insert_stream_fragment(terminal, fragment, kind)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn finish_streams(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<bool> {
        let reasoning_finished = self.finish_stream(terminal, StreamKind::Reasoning)?;
        let assistant_finished = self.finish_stream(terminal, StreamKind::Assistant)?;
        self.current_stream_kind = None;
        self.stream_preview_deadline = None;
        self.live_stream_preview = None;
        Ok(reasoning_finished || assistant_finished)
    }

    fn finish_current_stream(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<bool> {
        if let Some(kind) = self.current_stream_kind {
            self.finish_stream(terminal, kind)
        } else {
            Ok(false)
        }
    }

    fn finish_stream(
        &mut self,
        terminal: &mut DefaultTerminal,
        kind: StreamKind,
    ) -> std::io::Result<bool> {
        let fragment = match kind {
            StreamKind::Assistant => self.assistant_stream.finish(),
            StreamKind::Reasoning => self.reasoning_stream.finish(),
        };
        self.update_stream_preview_deadline(kind);
        if let Some(fragment) = fragment {
            self.live_stream_preview = None;
            self.insert_stream_fragment(terminal, fragment, kind)?;
            Ok(true)
        } else {
            Ok(false)
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
                .drain_preview_markdown(inner_width, self.assistant_stream_in_code_block),
            StreamKind::Reasoning => self.reasoning_stream.drain_preview(),
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

    fn insert_final_answer_suffix(
        &mut self,
        terminal: &mut DefaultTerminal,
        answer: &str,
    ) -> std::io::Result<()> {
        match final_answer_delta(self.assistant_stream.emitted_text(), answer) {
            FinalAnswerDelta::None => {}
            FinalAnswerDelta::Append(suffix) => {
                self.assistant_stream.push_delta(suffix);
                if let Some(fragment) = self.assistant_stream.finish() {
                    self.insert_stream_fragment(terminal, fragment, StreamKind::Assistant)?;
                }
            }
            FinalAnswerDelta::Mismatch => {
                self.replace_current_turn_assistant_transcript(answer);
            }
        }
        Ok(())
    }

    fn render_stream_preview_lines(
        &self,
        preview: &LiveStreamPreview,
        width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if preview.include_leading_blank {
            lines.push(Line::raw(""));
        }
        let mut text_lines = Vec::new();
        if matches!(preview.kind, StreamKind::Assistant) {
            let mut in_code_block = self.assistant_stream_in_code_block;
            push_wrapped_markdown_without_copy_button(
                &mut text_lines,
                &preview.text,
                padded_content_width(width),
                &mut in_code_block,
            );
        } else {
            push_wrapped_text(
                &mut text_lines,
                &preview.text,
                padded_content_width(width),
                preview.kind.style(),
                LineFill::Natural,
            );
        }
        lines.extend(text_lines.into_iter().map(pad_display_line));
        lines
    }

    fn insert_stream_fragment(
        &mut self,
        terminal: &mut DefaultTerminal,
        fragment: StreamFragment,
        kind: StreamKind,
    ) -> std::io::Result<()> {
        let _ = terminal;
        let render_text = fragment.render_text();
        if !render_text.is_empty() {
            if matches!(kind, StreamKind::Assistant) {
                update_code_block_state(render_text, &mut self.assistant_stream_in_code_block);
            }
            self.last_inserted_was_tool = false;
        }
        let text = fragment.into_text();
        self.push_transcript_entry(kind.entry(text));
        Ok(())
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
        self.history_lines.invalidate_from(*first);
        for index in stale.iter().rev() {
            self.transcript.remove(*index);
        }
    }

    async fn execute_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Exit => self.execute_exit_command(),
            CommandId::New => self.execute_new_command(terminal, agent),
            CommandId::Model => {
                self.execute_model_command(invocation, terminal, agent)
                    .await
            }
            CommandId::TitleModel => self.execute_title_model_command(invocation, terminal),
            CommandId::RefreshModelList => {
                self.execute_refresh_model_list_command(invocation, terminal)
                    .await
            }
            CommandId::Login => {
                self.execute_login_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Logout => self.execute_logout_command(invocation, agent).await,
            CommandId::Resume => {
                self.execute_resume_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Config => self.execute_config_command(terminal),
            CommandId::Info => self.execute_info_command(),
            CommandId::Compact => self.execute_compact_command(terminal, agent).await,
            CommandId::Goal => self.execute_goal_command(invocation, terminal, agent).await,
            CommandId::Skills => self.execute_skills_command(),
            CommandId::Diff => self.execute_diff_command(),
            CommandId::Doctor => self.execute_doctor_command(),
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::Limits => self.execute_limits_command(terminal).await,
        }
    }

    async fn execute_compact_command(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        if let Ok(config) = self.info.config_repository.load() {
            agent.set_compaction_config((&config).into());
        }
        self.steering_prompts.clear();
        self.status = "compacting context".into();
        self.running = true;
        self.loading_spinner.start();
        terminal.draw(|frame| self.draw(frame))?;

        let interrupt_requested = AtomicBool::new(false);
        let tool_call_active = AtomicBool::new(false);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let compacted = {
            let mut compact_future = Box::pin(agent.compact(move |event| {
                let _ = event_tx.send(event);
                Ok(())
            }));
            loop {
                tokio::select! {
                    result = &mut compact_future => break result,
                    Some(event) = event_rx.recv() => {
                        self.handle_queued_agent_event(event, terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                    _ = tokio::time::sleep_until(self.stream_sleep_deadline()) => {
                        match self.handle_running_terminal_events(
                            terminal,
                            &interrupt_requested,
                            &tool_call_active,
                            RunningInputMode::Compacting,
                        ) {
                            Ok(StreamControl::Interrupt) => {
                                break Err(crate::agent::AgentError::Provider(
                                    crate::model::ModelError::Interrupted,
                                ));
                            }
                            Ok(StreamControl::Continue | StreamControl::Resize) => {}
                            Err(err) => break Err(crate::agent::AgentError::Provider(err)),
                        }
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                }
            }
        };
        while let Ok(event) = event_rx.try_recv() {
            self.handle_queued_agent_event(event, terminal)?;
        }
        self.running = false;
        self.loading_spinner.stop();

        match compacted {
            Ok(true) => {
                self.insert_entry(&Entry::Notice("compacted conversation context".into()));
                self.status = "context compacted".into();
            }
            Ok(false) => {
                self.insert_entry(&Entry::Notice(
                        "not enough conversation history to compact, or the model context window is unknown"
                            .into(),
                    ),
                );
                self.status = "context not compacted".into();
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "failed to compact conversation context: {err}"
                )));
                self.status = "context compaction failed".into();
            }
        }
        Ok(())
    }

    fn execute_exit_command(&mut self) -> anyhow::Result<()> {
        self.insert_entry(&Entry::Notice("exiting rho".into()));
        self.should_quit = true;
        self.status = "exiting".into();
        Ok(())
    }

    fn execute_new_command(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        agent.reset();
        agent.set_session_id(None);
        agent.clear_history_sink();
        self.info.session_id = None;
        self.composer = ComposerMode::Input;
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.queued_prompts.clear();
        self.goal = None;
        self.steering_prompts.clear();
        self.reset_streams();
        self.running = false;
        self.active_tool_call = false;
        self.cumulative_usage = None;
        self.latest_usage = None;
        self.current_context = None;
        self.pending_session_title = None;
        self.current_turn_start = None;
        self.transcript.clear();
        self.history_lines.invalidate_from(0);
        self.last_inserted_was_tool = false;
        self.scroll_history_to_bottom();
        self.clamp_history_scroll_for_terminal(terminal)?;
        self.status = "new session".into();
        Ok(())
    }

    async fn execute_refresh_model_list_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let providers = if invocation.args.trim().is_empty() {
            self.refresh_available_auths();
            provider::providers()
                .iter()
                .filter(|provider| provider.model_refresh.is_some())
                .filter(|provider| {
                    self.available_auths
                        .iter()
                        .any(|auth| auth == provider.auth)
                })
                .map(|provider| provider.name.to_string())
                .collect()
        } else {
            vec![invocation.args.trim().to_string()]
        };

        if providers.is_empty() {
            self.insert_entry(&Entry::Notice(
                    "no refreshable providers are configured. run /login for a provider with model list support."
                        .into(),
                ),
            );
            self.status = "model refresh skipped".into();
            return Ok(());
        }

        self.status = "refreshing model list".into();
        terminal.draw(|frame| self.draw(frame))?;
        for provider in providers {
            match refresh_provider_models_with_store(&provider, self.credential_store.as_ref())
                .await
            {
                Ok(refresh) => {
                    self.insert_entry(&Entry::Notice(format!(
                        "refreshed {} model list: {} models",
                        refresh.provider,
                        refresh.models.len()
                    )));
                }
                Err(err) => {
                    self.insert_entry(&Entry::Error(format!(
                        "failed to refresh {provider} model list: {err}"
                    )));
                }
            }
        }
        self.status = "model list refresh complete".into();
        Ok(())
    }

    async fn execute_model_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let model = invocation.args.trim();
        if model.is_empty() {
            self.open_model_picker(terminal, agent).await?;
            return Ok(());
        }

        self.refresh_available_auths();
        match catalog::resolve_model_selection_for_auths(
            model,
            &self.info.provider,
            &self.info.auth,
            &self.available_auths,
        ) {
            Ok(selection) => self.select_model(selection, agent),
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "model switch failed".into();
                Ok(())
            }
        }
    }

    async fn open_model_picker(
        &mut self,
        terminal: &mut DefaultTerminal,
        _agent: &mut Agent,
    ) -> anyhow::Result<()> {
        self.status = "loading models".into();
        terminal.draw(|frame| self.draw(frame))?;
        self.refresh_available_auths();
        let picker = model_picker::model_picker(&self.info, &self.available_auths);

        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no cached API models. run /refresh-model-list after signing in.".into(),
            ));
            self.status = "ready".into();
            return Ok(());
        }

        self.composer = ComposerMode::Picker(picker);
        self.status = "select model".into();
        Ok(())
    }

    fn execute_title_model_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let model = invocation.args.trim();
        if model.is_empty() {
            return self.open_title_model_picker(terminal);
        }

        self.refresh_available_auths();
        let (provider, _model, auth) = self.title_model_selection();
        match catalog::resolve_model_selection_for_auths(
            model,
            &provider,
            &auth,
            &self.available_auths,
        ) {
            Ok(selection) => self.select_title_model(selection),
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "title model switch failed".into();
                Ok(())
            }
        }
    }

    fn open_title_model_picker(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        self.status = "loading title models".into();
        terminal.draw(|frame| self.draw(frame))?;
        self.refresh_available_auths();
        let (provider, model, _auth) = self.title_model_selection();
        let picker = model_picker::title_model_picker(
            &provider,
            &model,
            &self.info.favorite_models,
            &self.available_auths,
        );

        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no cached API models. run /refresh-model-list after signing in.".into(),
            ));
            self.status = "ready".into();
            return Ok(());
        }

        self.composer = ComposerMode::Picker(picker);
        self.status = "select title model".into();
        Ok(())
    }

    async fn submit_picker_selection(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            self.composer = ComposerMode::Input;
            self.status = "ready".into();
            return Ok(());
        };

        if !matches!(action, PickerAction::Config) {
            self.composer = ComposerMode::Input;
        }
        match action {
            PickerAction::SelectModel => {
                self.refresh_available_auths();
                match catalog::resolve_model_selection_for_auths(
                    &value,
                    &self.info.provider,
                    &self.info.auth,
                    &self.available_auths,
                ) {
                    Ok(selection) => self.select_model(selection, agent),
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "model switch failed".into();
                        Ok(())
                    }
                }
            }
            PickerAction::SelectTitleModel => {
                self.refresh_available_auths();
                let (provider, _model, auth) = self.title_model_selection();
                match catalog::resolve_model_selection_for_auths(
                    &value,
                    &provider,
                    &auth,
                    &self.available_auths,
                ) {
                    Ok(selection) => self.select_title_model(selection),
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "title model switch failed".into();
                        Ok(())
                    }
                }
            }
            PickerAction::LoginProvider => {
                self.start_login_for_provider(&value, terminal, agent).await
            }
            PickerAction::LogoutProvider => self.logout_provider(&value, agent).await,
            PickerAction::InsertSkillCommand => {
                self.input = format!("/skill:{value}");
                self.input_cursor = self.input_char_len();
                self.command_palette_dismissed = true;
                self.status = "skill command inserted".into();
                Ok(())
            }
            PickerAction::ResumeSession => {
                self.submit_resume_selection(&value, terminal, agent).await
            }
            PickerAction::Config => self.submit_config_selection(&value, agent),
            PickerAction::Doctor => Ok(()),
        }
    }

    fn submit_config_selection(&mut self, value: &str, agent: &mut Agent) -> anyhow::Result<()> {
        match value {
            config_picker::REASONING_VALUE => self.cycle_reasoning(agent),
            config_picker::SHOW_REASONING_OUTPUT_VALUE => self.toggle_reasoning_output(),
            config_picker::CHECK_FOR_UPDATES_VALUE => self.toggle_check_for_updates(),
            config_picker::AUTO_COMPACT_VALUE => self.toggle_auto_compact(),
            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactThresholdPercent,
                    config.compact_threshold_percent as usize,
                ));
                self.status = "edit compact threshold percent".into();
                Ok(())
            }
            config_picker::COMPACT_TARGET_PERCENT_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactTargetPercent,
                    config.compact_target_percent as usize,
                ));
                self.status = "edit compact target percent".into();
                Ok(())
            }
            config_picker::MAX_OUTPUT_BYTES_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxOutputBytes,
                    config.max_output_bytes,
                ));
                self.status = "edit max output bytes".into();
                Ok(())
            }
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxToolOutputLines,
                    config.max_tool_output_lines,
                ));
                self.status = "edit max tool output lines".into();
                Ok(())
            }
            config_picker::INLINE_SHELL_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::Picker(config_picker::inline_shell_picker(&config));
                self.status = "select inline shell".into();
                Ok(())
            }
            value if value.starts_with(config_picker::INLINE_SHELL_PREFIX) => {
                let shell = value[config_picker::INLINE_SHELL_PREFIX.len()..].to_string();
                self.info.config_repository.update(|config| {
                    config.inline_shell.clone_from(&shell);
                })?;
                self.open_main_config_picker_selected(config_picker::INLINE_SHELL_VALUE)?;
                self.status = format!("inline shell: {shell}");
                Ok(())
            }
            config_picker::WEB_SEARCH_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer = ComposerMode::Picker(config_picker::web_search_config_picker(
                    &config,
                    self.credential_store.as_ref(),
                ));
                self.status = "web search config".into();
                Ok(())
            }
            config_picker::WEB_SEARCH_BACK_VALUE => {
                let config = self.info.config_repository.load()?;
                self.composer =
                    ComposerMode::Picker(config_picker::config_picker(&self.info, &config));
                self.status = "config".into();
                Ok(())
            }
            config_picker::WEB_SEARCH_PROVIDER_VALUE => self.cycle_web_search_provider(),
            config_picker::WEB_SEARCH_OPENAI_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::OpenAiSearch)
            }
            config_picker::WEB_SEARCH_EXA_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::Exa)
            }
            config_picker::WEB_SEARCH_BRAVE_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::Brave)
            }
            _ => Ok(()),
        }
    }

    fn open_web_search_api_key_editor(&mut self, key: ConfigTextKey) -> anyhow::Result<()> {
        let credential = key.web_search_credential();
        let config = self.info.config_repository.load()?;
        let (value, load_error) = resolve_web_search_editor_value(
            load_web_search_api_key(self.credential_store.as_ref(), credential),
            config.legacy_web_search_api_key(credential),
        );
        if let Some(err) = load_error {
            self.insert_entry(&Entry::Error(format!(
                "could not access {}: {err}",
                key.label()
            )));
        }
        self.composer = ComposerMode::ConfigTextInput(ConfigTextInput::new(key, value));
        self.status = format!("edit {}", key.label());
        Ok(())
    }

    fn refresh_main_config_picker(&mut self, selected_value: &str) -> anyhow::Result<()> {
        let filter = match &self.composer {
            ComposerMode::Picker(picker) => picker.filter.clone(),
            _ => String::new(),
        };
        self.open_main_config_picker(selected_value, filter)
    }

    fn open_main_config_picker_selected(&mut self, selected_value: &str) -> anyhow::Result<()> {
        self.open_main_config_picker(selected_value, String::new())
    }

    fn open_main_config_picker(
        &mut self,
        selected_value: &str,
        filter: String,
    ) -> anyhow::Result<()> {
        let config = self.info.config_repository.load()?;
        let mut picker = config_picker::config_picker(&self.info, &config);
        Self::restore_picker_position(&mut picker, selected_value, filter);
        self.composer = ComposerMode::Picker(picker);
        self.status = "config".into();
        Ok(())
    }

    fn refresh_web_search_config_picker(&mut self, selected_value: &str) -> anyhow::Result<()> {
        let filter = match &self.composer {
            ComposerMode::Picker(picker) => picker.filter.clone(),
            _ => String::new(),
        };
        let config = self.info.config_repository.load()?;
        let mut picker =
            config_picker::web_search_config_picker(&config, self.credential_store.as_ref());
        Self::restore_picker_position(&mut picker, selected_value, filter);
        self.composer = ComposerMode::Picker(picker);
        Ok(())
    }

    fn handle_picker_escape(&mut self, running: bool) -> anyhow::Result<()> {
        if self.web_search_config_picker_is_open() || self.inline_shell_picker_is_open() {
            let selected = if self.web_search_config_picker_is_open() {
                config_picker::WEB_SEARCH_VALUE
            } else {
                config_picker::INLINE_SHELL_VALUE
            };
            self.open_main_config_picker_selected(selected)
        } else {
            self.composer = ComposerMode::Input;
            self.status = if running { "running" } else { "ready" }.into();
            Ok(())
        }
    }

    fn model_picker_is_open(&self) -> bool {
        matches!(
            &self.composer,
            ComposerMode::Picker(picker)
                if matches!(
                    picker.action,
                    PickerAction::SelectModel | PickerAction::SelectTitleModel
                )
        )
    }

    fn toggle_selected_model_favorite(&mut self) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            return Ok(());
        };
        if !matches!(
            action,
            PickerAction::SelectModel | PickerAction::SelectTitleModel
        ) {
            return Ok(());
        }
        let Some(favorite) = favorites::favorite_model_from_value(&value) else {
            return Ok(());
        };

        let filter = match &self.composer {
            ComposerMode::Picker(picker) => picker.filter.clone(),
            _ => String::new(),
        };
        let save_result = self.info.config_repository.update(|config| {
            let pinned = favorites::toggle_favorite(
                &mut config.favorite_models,
                &favorite.provider,
                &favorite.model,
            );
            (pinned, config.favorite_models.clone())
        });
        let (pinned, favorite_models) = match save_result {
            Ok(saved) => saved,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save pinned models: {err}"
                )));
                self.status = "config save failed".into();
                return Ok(());
            }
        };
        self.info.favorite_models = favorite_models;

        self.refresh_available_auths();
        let mut picker = match action {
            PickerAction::SelectModel if self.running => model_picker::model_picker_during_run(
                &self.info,
                self.pending_model_selection.as_ref(),
                &self.available_auths,
            ),
            PickerAction::SelectModel => {
                model_picker::model_picker(&self.info, &self.available_auths)
            }
            PickerAction::SelectTitleModel => {
                let (provider, model, _auth) = self.title_model_selection();
                model_picker::title_model_picker(
                    &provider,
                    &model,
                    &self.info.favorite_models,
                    &self.available_auths,
                )
            }
            PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::InsertSkillCommand
            | PickerAction::ResumeSession
            | PickerAction::Config
            | PickerAction::Doctor => return Ok(()),
        };
        Self::restore_picker_position(&mut picker, &value, filter);
        self.composer = ComposerMode::Picker(picker);
        let action = if pinned { "pinned" } else { "unpinned" };
        self.insert_entry(&Entry::Notice(format!("{action} {value}")));
        self.status = format!("{action} model");
        Ok(())
    }

    fn web_search_config_picker_is_open(&self) -> bool {
        matches!(
            &self.composer,
            ComposerMode::Picker(picker)
                if picker
                    .items
                    .iter()
                    .any(|item| item.value == config_picker::WEB_SEARCH_BACK_VALUE)
        )
    }

    fn picker_space_confirms_selection(&self) -> bool {
        matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action.space_confirms_selection()
        )
    }

    fn restore_picker_position(picker: &mut UiPicker, selected_value: &str, filter: String) {
        picker.filter = filter;
        if let Some(index) = picker
            .items
            .iter()
            .position(|item| item.value == selected_value)
        {
            picker.selected = index;
            if picker.selected_item().is_some() {
                return;
            }
        }
        picker.filter.clear();
        if let Some(index) = picker
            .items
            .iter()
            .position(|item| item.value == selected_value)
        {
            picker.selected = index;
        } else {
            picker.select_first_match();
        }
    }

    fn cycle_reasoning(&mut self, agent: &mut Agent) -> anyhow::Result<()> {
        let supported_reasoning = crate::model::models_dev::cached_reasoning_levels(
            &self.info.provider,
            &self.info.model,
        );
        let reasoning = self
            .info
            .reasoning
            .next_supported(supported_reasoning.as_deref());
        if !agent.set_provider_reasoning(reasoning) {
            let provider = match build_provider(&self.info.provider, &self.info.model, reasoning) {
                Ok(provider) => provider,
                Err(err) => {
                    self.insert_entry(&Entry::Error(format!(
                        "could not update reasoning to {reasoning}: {err}"
                    )));
                    self.status = "reasoning change failed".into();
                    return Ok(());
                }
            };
            agent.replace_provider(provider);
        }
        self.info.reasoning = reasoning;
        self.info.diagnostics.update_identity(
            &self.info.provider,
            &self.info.model,
            self.info.reasoning,
        );
        let save_result = self.info.config_repository.update(|config| {
            config.reasoning = reasoning;
        });
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            let config = self.info.config_repository.load().unwrap_or_default();
            self.info.show_reasoning_output = config.show_reasoning_output;
            self.refresh_main_config_picker(config_picker::REASONING_VALUE)?;
        }
        match save_result {
            Ok(()) => {
                self.status = format!("reasoning: {reasoning}");
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                        "reasoning set to {reasoning} for this session, but saving config failed: {err}"
                    )),
                );
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    fn toggle_check_for_updates(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(&self.info.config_repository, ConfigToggle::CheckForUpdates) {
            Ok(ConfigMutation::CheckForUpdates(check_for_updates)) => {
                self.info
                    .diagnostics
                    .update_check_for_updates(check_for_updates);
                if !check_for_updates {
                    self.info.update_notice = None;
                }
                self.status = if check_for_updates {
                    "check for updates: on".into()
                } else {
                    "check for updates: off".into()
                };
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save update check setting: {err}"
                )));
                self.status = "config save failed".into();
            }
            Ok(
                ConfigMutation::AutoCompact(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            self.refresh_main_config_picker(config_picker::CHECK_FOR_UPDATES_VALUE)?;
        }
        Ok(())
    }

    fn toggle_auto_compact(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(&self.info.config_repository, ConfigToggle::AutoCompact) {
            Ok(ConfigMutation::AutoCompact(auto_compact)) => {
                self.status = if auto_compact {
                    "auto compact: on".into()
                } else {
                    "auto compact: off".into()
                };
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save auto compact setting: {err}"
                )));
                self.status = "config save failed".into();
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            self.refresh_main_config_picker(config_picker::AUTO_COMPACT_VALUE)?;
        }
        Ok(())
    }

    fn toggle_reasoning_output(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.config_repository,
            ConfigToggle::ShowReasoningOutput,
        ) {
            Ok(ConfigMutation::ShowReasoningOutput(show_reasoning_output)) => {
                self.info.show_reasoning_output = show_reasoning_output;
                self.status = if show_reasoning_output {
                    "reasoning output: shown".into()
                } else {
                    "reasoning output: hidden".into()
                };
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save reasoning output setting: {err}"
                )));
                self.status = "config save failed".into();
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            let config = self.info.config_repository.load().unwrap_or_default();
            self.info.show_reasoning_output = config.show_reasoning_output;
            self.refresh_main_config_picker(config_picker::SHOW_REASONING_OUTPUT_VALUE)?;
        }
        Ok(())
    }

    fn cycle_web_search_provider(&mut self) -> anyhow::Result<()> {
        let ConfigMutation::WebSearchProvider(provider) =
            config_editor::cycle_web_search_provider(&self.info.config_repository)?
        else {
            unreachable!("provider cycle returned a mismatched config mutation");
        };
        self.refresh_web_search_config_picker(config_picker::WEB_SEARCH_PROVIDER_VALUE)?;
        self.status = format!("web search: {provider}");
        Ok(())
    }

    fn active_picker_selection(&self) -> Option<(PickerAction, String)> {
        let ComposerMode::Picker(picker) = &self.composer else {
            return None;
        };
        picker
            .selected_item()
            .map(|item| (picker.action, item.value.clone()))
    }

    fn select_model(&mut self, selection: ModelSelection, agent: &mut Agent) -> anyhow::Result<()> {
        let provider = selection.provider;
        let model = selection.model;
        let auth = selection.auth;
        let provider_model = format!("{provider}/{model}");
        let supported_reasoning =
            crate::model::models_dev::cached_reasoning_levels(&provider, &model);
        let reasoning = self
            .info
            .reasoning
            .normalize(supported_reasoning.as_deref());
        let new_provider = match build_provider(&provider, &model, reasoning) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not switch to {provider_model}: {err}"
                )));
                self.status = "model switch failed".into();
                return Ok(());
            }
        };

        let handoff = agent.replace_provider(new_provider);
        if handoff.has_omissions() {
            let kinds = handoff.omitted_kinds.join(", ");
            self.insert_entry(&Entry::Notice(format!(
                "model handoff omitted {} nonportable provider context block(s): {kinds}; assistant text, tool history, and reasoning summaries were preserved",
                handoff.omitted_provider_context
            )));
        }
        self.info.provider = provider.clone();
        self.info.model = model.clone();
        self.info.reasoning = reasoning;
        self.info.auth = auth.clone();
        self.info.diagnostics.update_identity(
            &self.info.provider,
            &self.info.model,
            self.info.reasoning,
        );
        self.info.auth_unavailable = None;
        self.using_unavailable_provider = false;
        self.start_model_metadata_fetch(agent);
        match self.info.config_repository.update(|config| {
            config.provider = provider.clone();
            config.model = model.clone();
            config.reasoning = reasoning;
            config.auth = auth.clone();
        }) {
            Ok(()) => {
                self.insert_entry(&Entry::Notice(format!(
                        "model switched to {provider_model} with reasoning {reasoning} and saved to config"
                    )),
                );
                self.status = format!("model: {provider_model}");
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                        "model switched to {provider_model} with reasoning {reasoning} for this session, but saving config failed: {err}"
                    )),
                );
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    fn select_title_model(&mut self, selection: ModelSelection) -> anyhow::Result<()> {
        let provider = selection.provider;
        let model = selection.model;
        let auth = selection.auth;
        let provider_model = format!("{provider}/{model}");
        self.info.title_provider = Some(provider.clone());
        self.info.title_model = Some(model.clone());
        self.info.title_auth = Some(auth.clone());
        match self.info.config_repository.update(|config| {
            config.title_provider = Some(provider.clone());
            config.title_model = Some(model.clone());
            config.title_auth = Some(auth.clone());
        }) {
            Ok(()) => {
                self.insert_entry(&Entry::Notice(format!(
                    "session title model switched to {provider_model} and saved to config"
                )));
                self.status = format!("title model: {provider_model}");
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                        "session title model switched to {provider_model} for this session, but saving config failed: {err}"
                    )),
                );
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    fn refresh_available_auths(&mut self) {
        self.available_auths = available_auth_modes(self.credential_store.as_ref());
    }

    fn save_current_config(&self) -> anyhow::Result<()> {
        self.info.config_repository.update(|config| {
            config.provider = self.info.provider.clone();
            config.model = self.info.model.clone();
            config.auth = self.info.auth.clone();
        })
    }

    async fn execute_resume_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let session_id = invocation.args.trim();
        if !session_id.is_empty() {
            return self
                .submit_resume_selection(session_id, terminal, agent)
                .await;
        }

        self.open_resume_picker()
    }

    fn open_resume_picker(&mut self) -> anyhow::Result<()> {
        match Session::list(&self.info.cwd) {
            Ok(sessions) if sessions.is_empty() => {
                self.insert_entry(&Entry::Notice(
                    "no saved sessions for this workspace".into(),
                ));
                self.status = "no sessions".into();
            }
            Ok(sessions) => {
                let picker =
                    session_picker::session_picker(sessions, self.info.session_id.as_deref());
                if picker.items.is_empty() {
                    self.insert_entry(&Entry::Notice(
                        "no other saved sessions for this workspace".into(),
                    ));
                    self.status = "no sessions".into();
                    return Ok(());
                }
                self.composer = ComposerMode::Picker(picker);
                self.status = "select session".into();
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!("could not list sessions: {err}")));
                self.status = "resume failed".into();
            }
        }
        Ok(())
    }

    async fn submit_resume_selection(
        &mut self,
        session_id: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        match self.resume_session_by_id(session_id, terminal, agent) {
            Ok(()) => {
                self.info
                    .herdr
                    .report_session(self.info.session_id.as_deref())
                    .await;
                Ok(())
            }
            Err(err) => {
                self.composer = ComposerMode::Input;
                self.insert_entry(&Entry::Error(format!("could not resume session: {err}")));
                self.status = "resume failed".into();
                Ok(())
            }
        }
    }

    fn resume_session_by_id(
        &mut self,
        session_id: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let (session, histories) = Session::open_by_id_with_histories(&self.info.cwd, session_id)?;
        let full_id = session.id().to_string();
        let short_id = short_session_id(&full_id);

        let display_history = histories.display;
        agent.replace_history(histories.model);
        agent.set_session_id(Some(full_id.clone()));
        agent.set_history_sink(SessionHistorySink::new(session));
        self.info.session_id = Some(full_id);
        self.info.recovered_messages = display_history.clone();
        self.composer = ComposerMode::Input;
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.reset_streams();
        self.running = false;
        self.goal = None;
        self.cumulative_usage = None;
        self.latest_usage = None;
        self.current_context = None;
        let entries = transcript_entries_from_messages(&display_history);
        let width = terminal.size()?.width as usize;
        let (_omitted, visible_entries) = recovered_history_tail(
            &entries,
            width,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.max_tool_output_lines,
        );
        self.transcript = visible_entries;
        self.history_lines.invalidate_from(0);
        self.last_inserted_was_tool = self.transcript.last().is_some_and(is_tool_entry);
        self.scroll_history_to_bottom();
        self.clamp_history_scroll_for_terminal(terminal)?;
        self.insert_entry(&Entry::Notice(format!("resumed session {short_id}")));
        self.status = format!("resumed {short_id}");
        Ok(())
    }

    fn execute_config_command(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let config = self.info.config_repository.load()?;
        self.info.max_tool_output_lines = config.max_tool_output_lines.max(1);
        self.info
            .diagnostics
            .update_max_tool_output_lines(self.info.max_tool_output_lines);
        self.info.show_reasoning_output = config.show_reasoning_output;
        self.composer = ComposerMode::Picker(config_picker::config_picker(&self.info, &config));
        self.status = "config".into();
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }

    fn execute_info_command(&mut self) -> anyhow::Result<()> {
        let identity = self.info.diagnostics.identity();
        self.insert_entry(&Entry::Notice(format!(
            "rho {}\nprovider: {}\nmodel: {}\nreasoning: {}",
            identity.rho_version, identity.provider, identity.model, identity.reasoning
        )));
        self.status = "runtime info".into();
        Ok(())
    }

    fn execute_skills_command(&mut self) -> anyhow::Result<()> {
        let picker = skill_picker::skill_picker(crate::skills::discover(&self.info.cwd));
        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice("no skills loaded".into()));
            self.status = "skills".into();
            return Ok(());
        }

        self.composer = ComposerMode::Picker(picker);
        self.status = "select skill".into();
        Ok(())
    }

    fn execute_skill_command(&mut self, name: &str, agent: &mut Agent) -> anyhow::Result<bool> {
        let Some(name) = name.strip_prefix("skill:") else {
            return Ok(false);
        };
        let Some(skill) = crate::skills::discover(&self.info.cwd)
            .into_iter()
            .find(|skill| skill.name == name)
        else {
            return Ok(false);
        };

        self.ensure_session(agent)?;
        agent.load_skill(&skill)?;
        self.insert_entry(&Entry::Notice(format!(
            "loaded skill {} from {}",
            skill.name, skill.source
        )));
        self.status = format!("loaded skill {}", skill.name);
        Ok(true)
    }

    fn toggle_latest_tool_output(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        if let Some(pending) = self.pending_tool_call.as_mut() {
            if tool_display_line_count(&pending.display_lines) <= self.info.max_tool_output_lines {
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

        let Some(index) = self
            .transcript
            .iter()
            .rposition(|entry| expandable_tool_entry(entry, self.info.max_tool_output_lines))
        else {
            self.status = "no truncated tool output".into();
            return Ok(());
        };

        let expand =
            !matches!(self.transcript.get(index), Some(Entry::Tool(tool)) if tool.expanded);
        for entry in &mut self.transcript {
            if let Entry::Tool(tool) = entry {
                tool.expanded = false;
            }
        }
        if let Some(Entry::Tool(tool)) = self.transcript.get_mut(index) {
            tool.expanded = expand;
            self.history_lines.invalidate_from(index);
        }
        self.status = if expand {
            "tool output expanded".into()
        } else {
            "tool output collapsed".into()
        };
        self.clamp_history_scroll_for_terminal(terminal)
    }

    fn record_agent_event(&mut self, event: AgentEvent) -> Option<Entry> {
        match event {
            AgentEvent::StepStarted(step) => {
                self.reset_streams();
                self.hidden_reasoning_active = !self.active_turn_show_reasoning_output;
                self.running = true;
                self.active_tool_call = false;
                self.pending_tool_call = None;
                self.loading_spinner.start_if_needed();
                self.status = format!("running step {step}");
                None
            }
            AgentEvent::ToolStarted { display_lines, .. } => {
                self.active_tool_call = true;
                self.pending_tool_call = Some(ToolEntry {
                    state: ToolEntryState::Running,
                    display_lines,
                    expanded: false,
                });
                None
            }
            AgentEvent::ToolUpdated { display_lines } => {
                let expanded = self
                    .pending_tool_call
                    .as_ref()
                    .is_some_and(|pending| pending.expanded);
                self.pending_tool_call = Some(ToolEntry {
                    state: ToolEntryState::Running,
                    display_lines,
                    expanded,
                });
                None
            }
            AgentEvent::ToolCallUpdated { display_lines } if !self.active_tool_call => {
                self.pending_tool_call = (!display_lines.is_empty()).then_some(ToolEntry {
                    state: ToolEntryState::Running,
                    display_lines,
                    expanded: false,
                });
                None
            }
            AgentEvent::ToolCallUpdated { .. } => None,
            AgentEvent::OutputDelta(_) | AgentEvent::ReasoningDelta(_) => None,
            AgentEvent::ContextUsage(usage) => {
                self.current_context = Some(usage);
                None
            }
            AgentEvent::Usage(usage) => {
                let usage = usage_with_estimated_cost(usage, self.model_metadata.as_ref());
                self.latest_usage = Some(usage.clone());
                merge_usage(&mut self.cumulative_usage, usage);
                None
            }
            AgentEvent::ToolFinished {
                ok,
                display_style,
                display_lines,
                ..
            } => {
                self.statusline.refresh_git_branch();
                self.active_tool_call = false;
                let expanded = self
                    .pending_tool_call
                    .as_ref()
                    .is_some_and(|pending| pending.expanded);
                self.pending_tool_call = None;
                Some(Entry::Tool(ToolEntry {
                    state: ToolEntryState::Finished { ok, display_style },
                    display_lines,
                    expanded,
                }))
            }
            AgentEvent::QuestionnaireStarted(_) => None,
            AgentEvent::QuestionnaireFinished(_) => None,
        }
    }

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let now = Instant::now();
        let area = frame.area();
        let width = area.width as usize;
        let live_history = self.history_live_lines(width, now);
        let history_len = self
            .history_static_len(width)
            .saturating_add(live_history.len());
        let composer_lines = self.composer_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        let layout = self.screen_layout_for_history_len(
            area,
            history_len,
            &composer_lines,
            command_lines.len(),
        );
        let (history_start, history_count) =
            self.visible_history_window(history_len, layout.history.height as usize);
        let history_visible = self.visible_history_lines_with_live(
            width,
            history_start,
            history_count,
            &live_history,
        );
        frame.render_widget(
            Paragraph::new(history_visible).style(Style::default()),
            layout.history,
        );
        if let Some(selection) = self.text_selection {
            highlight_selection(frame.buffer_mut(), layout.history, history_start, selection);
        }
        if let Some(hovered_line) = self.hovered_code_block_copy {
            let code_block_copy_targets = self.code_block_copy_targets(width);
            if let Some(target) = code_block_copy_targets
                .iter()
                .find(|target| target.line == hovered_line)
                .filter(|target| {
                    (history_start..history_start + layout.history.height as usize)
                        .contains(&target.line)
                })
            {
                let row = layout
                    .history
                    .y
                    .saturating_add(target.line.saturating_sub(history_start) as u16);
                for column in target.columns.clone().take(layout.history.width as usize) {
                    frame.buffer_mut()[(layout.history.x.saturating_add(column as u16), row)]
                        .set_style(Theme::markdown_code_copy_button(/*hovered*/ true));
                }
            }
        }
        if let Some(scrollbar) = layout
            .history_scrollbar
            .filter(|_| self.should_render_history_scrollbar(now))
        {
            scrollbar.render(frame, self.history_scrollbar_drag.is_some());
        }
        if let Some(activity) = layout.activity {
            frame.render_widget(
                Paragraph::new(self.loading_spinner.line(now, activity.width as usize))
                    .style(Style::default()),
                activity,
            );
        }
        if let Some(button) = layout.jump_to_bottom {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    self.jump_to_bottom_text(width),
                    Theme::jump_to_bottom(),
                ))
                .style(Style::default()),
                button,
            );
        }
        if layout.top_divider.height > 0 {
            frame.render_widget(
                Paragraph::new(vec![self.divider_line(width)]).style(Style::default()),
                layout.top_divider,
            );
        }

        let composer_visible = composer_lines
            .into_iter()
            .skip(layout.composer_start)
            .take(layout.composer.height as usize)
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(composer_visible).style(Style::default()),
            layout.composer,
        );
        if layout.bottom_divider.height > 0 {
            frame.render_widget(
                Paragraph::new(vec![self.divider_line(width)]).style(Style::default()),
                layout.bottom_divider,
            );
        }
        let statusline_height = layout.statusline.height as usize;
        for (index, line) in self
            .statusline_lines(width)
            .iter()
            .take(statusline_height)
            .enumerate()
        {
            let row = Rect::new(
                layout.statusline.x,
                layout.statusline.y.saturating_add(index as u16),
                layout.statusline.width,
                1,
            );
            frame.render_widget(line, row);
        }
        frame.render_widget(
            Paragraph::new(
                command_lines
                    .into_iter()
                    .take(layout.commands.height as usize)
                    .collect::<Vec<_>>(),
            )
            .style(Style::default()),
            layout.commands,
        );
        if let Some(notice) = &self.copy_notice {
            render_copy_notice(frame, area, notice, now);
        }

        let full_cursor = self.composer_cursor_position(width);
        let max_cursor_x = width.max(1).saturating_sub(1) as u16;
        let composer_height = layout.composer.height.max(1);
        let cursor_y = full_cursor
            .y
            .saturating_sub(layout.composer_start as u16)
            .min(composer_height.saturating_sub(1));
        frame.set_cursor_position(Position {
            x: layout
                .composer
                .x
                .saturating_add(full_cursor.x.min(max_cursor_x)),
            y: layout.composer.y.saturating_add(cursor_y),
        });
    }

    #[cfg(test)]
    fn active_lines(&mut self, width: usize) -> Vec<Line<'static>> {
        self.active_lines_at_for_height(width, DEFAULT_TUI_HEIGHT as usize, Instant::now())
    }

    #[cfg(test)]
    fn active_lines_for_height(
        &mut self,
        width: usize,
        viewport_height: usize,
    ) -> Vec<Line<'static>> {
        self.active_lines_at_for_height(width, viewport_height, Instant::now())
    }

    #[cfg(test)]
    fn active_lines_at_for_height(
        &mut self,
        width: usize,
        viewport_height: usize,
        now: Instant,
    ) -> Vec<Line<'static>> {
        self.active_frame_at_for_height(width, viewport_height, now)
            .lines
    }

    #[cfg(test)]
    fn active_frame_at_for_height(
        &mut self,
        width: usize,
        viewport_height: usize,
        now: Instant,
    ) -> ActiveFrame {
        let area = Rect::new(0, 0, width as u16, viewport_height as u16);
        let history_len = self.history_len(width, now);
        let composer_lines = self.composer_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        let layout = self.screen_layout_for_history_len(
            area,
            history_len,
            &composer_lines,
            command_lines.len(),
        );
        let (history_start, history_count) =
            self.visible_history_window(history_len, layout.history.height as usize);
        let mut lines = self.visible_history_lines(width, now, history_start, history_count);
        lines.resize(layout.history.height as usize, Line::default());
        if let Some(activity) = layout.activity {
            lines[activity.y.saturating_sub(layout.history.y) as usize] =
                self.loading_spinner.line(now, activity.width as usize);
        }
        if let Some(button) = layout.jump_to_bottom {
            lines[button.y.saturating_sub(layout.history.y) as usize] =
                self.jump_to_bottom_line(width);
        }
        if layout.top_divider.height > 0 {
            lines.push(self.divider_line(width));
        }
        lines.extend(
            composer_lines
                .into_iter()
                .skip(layout.composer_start)
                .take(layout.composer.height as usize),
        );
        if layout.bottom_divider.height > 0 {
            lines.push(self.divider_line(width));
        }
        lines.extend(
            self.statusline_lines(width)
                .iter()
                .take(layout.statusline.height as usize)
                .cloned(),
        );
        lines.extend(
            command_lines
                .into_iter()
                .take(layout.commands.height as usize),
        );

        ActiveFrame { lines }
    }

    fn screen_layout(&mut self, area: Rect, now: Instant) -> ScreenLayout {
        let width = area.width as usize;
        let history_len = self.history_len(width, now);
        let composer_lines = self.composer_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        self.screen_layout_for_history_len(area, history_len, &composer_lines, command_lines.len())
    }

    fn screen_layout_for_history_len(
        &self,
        area: Rect,
        history_len: usize,
        composer_lines: &[Line<'_>],
        command_line_count: usize,
    ) -> ScreenLayout {
        let width = area.width as usize;
        let height = area.height as usize;
        let full_cursor = self.composer_cursor_position(width);
        let cursor_line = (full_cursor.y as usize).min(composer_lines.len().saturating_sub(1));
        let statusline_height = self.statusline.height().min(height);
        let bottom_divider_height = usize::from(height > statusline_height);
        let command_height = command_line_count
            .min(height.saturating_sub(statusline_height + bottom_divider_height));
        let bottom_fixed_height = bottom_divider_height + statusline_height + command_height;
        let available_above_bottom = height.saturating_sub(bottom_fixed_height);
        let show_top_divider = available_above_bottom > 1 && !composer_lines.is_empty();
        let history_height_without_jump =
            self.history_height_from_line_counts(height, composer_lines.len(), command_line_count);
        let show_jump_to_bottom = history_height_without_jump > 0
            && self.visible_history_start(history_len, history_height_without_jump)
                < history_len.saturating_sub(history_height_without_jump);
        let reserved_above_composer = usize::from(show_top_divider);
        let composer_budget = available_above_bottom.saturating_sub(reserved_above_composer);
        let visible_composer_len = composer_lines.len().min(composer_budget);
        let composer_start =
            visible_composer_start(cursor_line, composer_lines.len(), visible_composer_len);
        let history_height =
            available_above_bottom.saturating_sub(reserved_above_composer + visible_composer_len);

        let mut y = area.y;
        let history = Rect::new(area.x, y, area.width, history_height as u16);
        y = y.saturating_add(history.height);
        let activity_y = history.bottom().saturating_sub(1);
        let jump_text = show_jump_to_bottom.then(|| self.jump_to_bottom_text(width));
        let jump_width = jump_text.as_deref().map_or(0, display_width).min(width) as u16;
        let jump_to_bottom = jump_text.map(|_| {
            Rect::new(
                history
                    .x
                    .saturating_add(history.width.saturating_sub(jump_width)),
                activity_y,
                jump_width,
                1,
            )
        });
        let spinner_available = if jump_width > 0 {
            width.saturating_sub(jump_width as usize + 1)
        } else {
            width
        };
        let spinner_width = activity::spinner_width(spinner_available) as u16;
        let activity = (self.loading_active() && spinner_width > 0 && history.height > 0)
            .then(|| Rect::new(history.x, activity_y, spinner_width, 1));
        let top_divider = if show_top_divider {
            let rect = Rect::new(area.x, y, area.width, 1);
            y = y.saturating_add(1);
            rect
        } else {
            Rect::new(area.x, y, area.width, 0)
        };
        let composer = Rect::new(area.x, y, area.width, visible_composer_len as u16);
        y = y.saturating_add(composer.height);
        let bottom_divider = Rect::new(area.x, y, area.width, bottom_divider_height as u16);
        y = y.saturating_add(bottom_divider.height);
        let statusline = Rect::new(area.x, y, area.width, statusline_height as u16);
        y = y.saturating_add(statusline.height);
        let commands = Rect::new(area.x, y, area.width, command_height as u16);

        ScreenLayout {
            history,
            history_scrollbar: HistoryScrollbar::new(
                history,
                history_len,
                self.visible_history_start(history_len, history_height),
            ),
            activity,
            jump_to_bottom,
            top_divider,
            composer,
            bottom_divider,
            statusline,
            commands,
            composer_start,
            history_len,
        }
    }

    fn divider_line(&self, width: usize) -> Line<'static> {
        let divider_style = match &self.composer {
            ComposerMode::Input => match inline_shell::mode_when_idle(self.running, &self.input) {
                Some(InlineShellMode::IncludeInContext) => Theme::shell_context(),
                Some(InlineShellMode::ExcludeFromContext) => Theme::shell_local(),
                None => Theme::reasoning_input_border(self.info.reasoning),
            },
            ComposerMode::Picker(_) | ComposerMode::Questionnaire(_) => Theme::input_prompt(),
            ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::OAuthPending(_) => Theme::dim(),
        };
        Line::styled("─".repeat(width.max(1)), divider_style)
    }

    #[cfg(test)]
    fn history_lines(&mut self, width: usize, now: Instant) -> Vec<Line<'static>> {
        let history_len = self.history_len(width, now);
        self.visible_history_lines(width, now, 0, history_len)
    }

    fn session_header_lines(&mut self, width: usize) -> &[Line<'static>] {
        let update_notice = self.info.update_notice.clone();
        let stale = self
            .session_header_cache
            .as_ref()
            .is_none_or(|cache| cache.width != width || cache.update_notice != update_notice);
        if stale {
            self.session_header_cache = Some(SessionHeaderCache {
                width,
                update_notice,
                lines: session_header_lines(&self.info, width),
            });
        }
        &self.session_header_cache.as_ref().unwrap().lines
    }

    fn history_len(&mut self, width: usize, now: Instant) -> usize {
        self.history_static_len(width)
            .saturating_add(self.history_live_lines(width, now).len())
    }

    fn visible_history_lines(
        &mut self,
        width: usize,
        now: Instant,
        start: usize,
        count: usize,
    ) -> Vec<Line<'static>> {
        let live = self.history_live_lines(width, now);
        self.visible_history_lines_with_live(width, start, count, &live)
    }

    fn visible_history_lines_with_live(
        &mut self,
        width: usize,
        start: usize,
        count: usize,
        live: &[Line<'static>],
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if count == 0 {
            return lines;
        }

        let header_lines = self.session_header_lines(width).to_vec();
        let header_len = header_lines.len();
        if start < header_len {
            let header_count = count.min(header_len - start);
            lines.extend(header_lines[start..start + header_count].iter().cloned());
        }

        if lines.len() < count {
            let transcript_start = start.saturating_sub(header_len);
            let transcript_count = count - lines.len();
            self.history_lines.extend_visible_lines(
                &self.transcript,
                width,
                self.info.max_tool_output_lines,
                transcript_start,
                transcript_count,
                &mut lines,
            );
        }

        let static_len = header_len.saturating_add(self.cached_transcript_line_count(width));
        if lines.len() < count {
            let live_start = start.saturating_sub(static_len);
            lines.extend(
                live.iter()
                    .skip(live_start)
                    .take(count - lines.len())
                    .cloned(),
            );
        }
        lines
    }

    fn history_static_len(&mut self, width: usize) -> usize {
        self.session_header_lines(width)
            .len()
            .saturating_add(self.cached_transcript_line_count(width))
    }

    fn cached_transcript_line_count(&mut self, width: usize) -> usize {
        self.history_lines
            .line_count(&self.transcript, width, self.info.max_tool_output_lines)
    }

    fn code_block_copy_targets(&mut self, width: usize) -> Vec<CodeBlockCopyTarget> {
        let header_len = self.session_header_lines(width).len();
        self.history_lines
            .code_blocks(&self.transcript, width, self.info.max_tool_output_lines)
            .iter()
            .map(|block: &CachedCodeBlock| CodeBlockCopyTarget {
                line: header_len.saturating_add(block.line),
                columns: block.copy_columns.clone(),
                text: Arc::clone(&block.text),
            })
            .collect()
    }

    fn history_live_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(pending) = &self.pending_tool_call {
            if self.last_inserted_was_tool || self.transcript.last().is_some_and(is_tool_entry) {
                lines.push(Line::raw(""));
            }
            lines.extend(tool_entry_lines(
                pending,
                width,
                self.info.max_tool_output_lines,
            ));
        }
        if let Some(preview) = &self.live_stream_preview {
            lines.extend(self.render_stream_preview_lines(preview, width));
        }
        if self.hidden_reasoning_active {
            lines.push(Line::raw(""));
            lines.push(pad_display_line(styled_line(
                "Thinking...".into(),
                padded_content_width(width),
                StreamKind::Reasoning.style(),
                LineFill::Natural,
            )));
        }
        lines
    }

    fn visible_history_window(&self, history_len: usize, height: usize) -> (usize, usize) {
        let count = if self.loading_active() && matches!(self.history_scroll, HistoryScroll::Bottom)
        {
            height.saturating_sub(1)
        } else {
            height
        };
        (self.visible_history_start(history_len, count), count)
    }

    fn visible_history_start(&self, history_len: usize, height: usize) -> usize {
        let max_start = history_len.saturating_sub(height);
        match self.history_scroll {
            HistoryScroll::Bottom => max_start,
            HistoryScroll::Manual { top_line } => top_line.min(max_start),
        }
    }

    #[cfg(test)]
    fn should_show_jump_to_bottom(&mut self, width: usize, height: usize, now: Instant) -> bool {
        let history_len = self.history_len(width, now);
        let history_height = self.history_height_for_screen(width, height, now);
        history_height > 0
            && self.visible_history_start(history_len, history_height)
                < history_len.saturating_sub(history_height)
    }

    fn history_height_for_screen(&self, width: usize, height: usize, _now: Instant) -> usize {
        self.history_height_from_line_counts(
            height,
            self.composer_lines(width).len(),
            self.command_suggestion_lines(width).len(),
        )
    }

    fn history_height_from_line_counts(
        &self,
        height: usize,
        composer_line_count: usize,
        command_line_count: usize,
    ) -> usize {
        let statusline_height = self.statusline.height().min(height);
        let bottom_divider_height = usize::from(height > statusline_height);
        let command_height = command_line_count
            .min(height.saturating_sub(statusline_height + bottom_divider_height));
        let bottom_fixed_height = bottom_divider_height + statusline_height + command_height;
        let available_above_bottom = height.saturating_sub(bottom_fixed_height);
        let show_top_divider = available_above_bottom > 1 && composer_line_count > 0;
        let reserved_above_composer = usize::from(show_top_divider);
        let composer_budget = available_above_bottom.saturating_sub(reserved_above_composer);
        let visible_composer_len = composer_line_count.min(composer_budget);
        available_above_bottom.saturating_sub(reserved_above_composer + visible_composer_len)
    }

    fn scroll_history_to_bottom(&mut self) {
        self.history_scroll = HistoryScroll::Bottom;
        self.hide_history_scrollbar();
    }

    fn scroll_history_page_up(&mut self, width: usize, height: usize, now: Instant) {
        let page = self.history_height_for_screen(width, height, now).max(1);
        self.scroll_history_lines(width, height, now, -(page as isize));
    }

    fn scroll_history_page_down(&mut self, width: usize, height: usize, now: Instant) {
        let page = self.history_height_for_screen(width, height, now).max(1);
        self.scroll_history_lines(width, height, now, page as isize);
    }

    fn scroll_history_lines(&mut self, width: usize, height: usize, now: Instant, delta: isize) {
        let history_len = self.history_len(width, now);
        let composer_line_count = self.composer_lines(width).len();
        let command_line_count = self.command_suggestion_lines(width).len();
        let history_height =
            self.history_height_from_line_counts(height, composer_line_count, command_line_count);
        let max_start = history_len.saturating_sub(history_height);
        let current = self.visible_history_start(history_len, history_height);
        let next = current.saturating_add_signed(delta).min(max_start);
        self.history_scroll = scroll_state_for_top_line(history_len, history_height, next);
        if matches!(self.history_scroll, HistoryScroll::Bottom) {
            self.hide_history_scrollbar();
        }
    }

    fn reveal_history_scrollbar(&mut self, now: Instant) {
        self.history_scrollbar_visible_until = Some(now + HISTORY_SCROLLBAR_REVEAL_DURATION);
    }

    fn hide_history_scrollbar(&mut self) {
        self.history_scrollbar_drag = None;
        self.history_scrollbar_visible_until = None;
        self.history_scrollbar_hovered = false;
    }

    fn should_render_history_scrollbar(&self, now: Instant) -> bool {
        self.history_scrollbar_drag.is_some()
            || self.history_scrollbar_hovered
            || self
                .history_scrollbar_visible_until
                .is_some_and(|visible_until| now < visible_until)
    }

    fn update_history_scrollbar_hover(
        &mut self,
        scrollbar: Option<HistoryScrollbar>,
        column: u16,
        row: u16,
    ) {
        self.history_scrollbar_hovered =
            scrollbar.is_some_and(|scrollbar| scrollbar.contains(column, row));
    }

    fn clamp_history_scroll(&mut self, width: usize, height: usize, now: Instant) {
        if matches!(self.history_scroll, HistoryScroll::Bottom) {
            self.history_scrollbar_drag = None;
            return;
        }
        let history_len = self.history_len(width, now);
        let composer_line_count = self.composer_lines(width).len();
        let command_line_count = self.command_suggestion_lines(width).len();
        let history_height =
            self.history_height_from_line_counts(height, composer_line_count, command_line_count);
        let max_start = history_len.saturating_sub(history_height);
        if let HistoryScroll::Manual { top_line } = self.history_scroll {
            self.history_scroll =
                scroll_state_for_top_line(history_len, history_height, top_line.min(max_start));
            if matches!(self.history_scroll, HistoryScroll::Bottom) {
                self.hide_history_scrollbar();
            }
        }
    }

    fn clamp_history_scroll_for_terminal<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let size = terminal.size()?;
        self.clamp_history_scroll(size.width as usize, size.height as usize, Instant::now());
        Ok(())
    }

    #[cfg(test)]
    fn jump_to_bottom_line(&self, width: usize) -> Line<'static> {
        Line::styled(self.jump_to_bottom_text(width), Theme::jump_to_bottom())
    }

    fn jump_to_bottom_text(&self, width: usize) -> String {
        activity::jump_to_bottom_text(
            width,
            &self.info.keybindings.jump_to_bottom.to_string(),
            /*alongside_spinner*/ self.loading_active(),
        )
    }

    fn handle_history_key<B: Backend>(
        &mut self,
        key: KeyEvent,
        terminal: &mut Terminal<B>,
    ) -> Result<bool, B::Error> {
        let size = terminal.size()?;
        let width = size.width as usize;
        let height = size.height as usize;
        let now = Instant::now();
        match (key.modifiers, key.code) {
            (_, KeyCode::PageUp) => {
                self.reveal_history_scrollbar(now);
                self.history_scrollbar_drag = None;
                self.scroll_history_page_up(width, height, now);
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::PageDown) => {
                self.reveal_history_scrollbar(now);
                self.history_scrollbar_drag = None;
                self.scroll_history_page_down(width, height, now);
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ if self.info.keybindings.jump_to_bottom.matches(key) => {
                self.scroll_history_to_bottom();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn composer_lines(&self, width: usize) -> Vec<Line<'static>> {
        match &self.composer {
            ComposerMode::Input => {
                let mut lines = input_lines_with_images(&self.input, &self.pending_images, width);
                if let Some(mode) = inline_shell::mode_when_idle(self.running, &self.input) {
                    let style = match mode {
                        InlineShellMode::IncludeInContext => Theme::shell_context(),
                        InlineShellMode::ExcludeFromContext => Theme::shell_local(),
                    };
                    for line in &mut lines {
                        *line = line.clone().style(style);
                    }
                }
                lines
            }
            ComposerMode::Picker(picker) => picker_lines(picker, width),
            ComposerMode::SecretInput(secret) => secret_input_lines(secret, width),
            ComposerMode::ConfigNumberInput(input) => config_number_input_lines(input, width),
            ComposerMode::ConfigTextInput(input) => config_text_input_lines(input, width),
            ComposerMode::OAuthPending(target) => oauth_pending_lines(target, width),
            ComposerMode::Questionnaire(questionnaire) => questionnaire_lines(questionnaire, width),
        }
    }

    fn goal_status(&self) -> Option<GoalStatus> {
        self.goal.as_ref().map(|goal| GoalStatus {
            turns: goal.turns,
            elapsed: goal.elapsed(),
        })
    }

    fn refresh_statusline_state(&mut self) {
        self.statusline.update_model(&self.info);
        self.statusline.update_usage(
            self.cumulative_usage.as_ref(),
            self.latest_usage.as_ref(),
            self.current_context.as_ref(),
        );
        self.statusline.update_model_metadata(
            self.model_metadata.as_ref(),
            self.pending_model_metadata.is_some(),
        );
    }

    fn statusline_lines(&mut self, width: usize) -> &[Line<'static>] {
        let goal = self.goal_status();
        self.refresh_statusline_state();
        self.statusline.lines(width, goal)
    }

    fn composer_cursor_position(&self, width: usize) -> Position {
        match &self.composer {
            ComposerMode::Input => {
                let mut position = input_cursor_position(&self.input, self.input_cursor, width);
                position.y = position.y.saturating_add(self.pending_images.len() as u16);
                position
            }
            ComposerMode::SecretInput(secret) => Position {
                x: char_prefix_display_width(&secret.value, secret.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigNumberInput(input) => Position {
                x: char_prefix_display_width(&input.value, input.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigTextInput(input) => Position {
                x: char_prefix_display_width(&input.value, input.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire_cursor_position(questionnaire, width)
            }
            ComposerMode::OAuthPending(_) => Position { x: 0, y: 0 },
            ComposerMode::Picker(picker) => Position {
                x: display_width(&picker.filter)
                    .saturating_add(2)
                    .min(width.saturating_sub(1)) as u16,
                y: 0,
            },
        }
    }

    fn command_suggestion_lines(&self, width: usize) -> Vec<Line<'static>> {
        if let Some((text, style)) = inline_shell::mode_hint_when_idle(self.running, &self.input) {
            return vec![styled_line(
                truncate_one_line(text, width.max(1)),
                width.max(1),
                style,
                LineFill::Natural,
            )];
        }
        if self.command_palette_visible() {
            let matches = self.command_matches();
            let selected_index = self.command_selection.min(matches.len().saturating_sub(1));
            let start = selected_index
                .saturating_add(1)
                .saturating_sub(MAX_COMMAND_SUGGESTIONS);

            return matches
                .into_iter()
                .enumerate()
                .skip(start)
                .take(MAX_COMMAND_SUGGESTIONS)
                .map(|(index, command)| {
                    let selected = index == selected_index;
                    let marker = if selected { ">" } else { " " };
                    let usage_width = 16usize.min(width.saturating_sub(5).max(1));
                    let description_width = width.saturating_sub(usage_width + 3).max(1);
                    let usage = truncate_one_line(&command.usage, usage_width);
                    let description = truncate_one_line(&command.description, description_width);
                    let usage_padding =
                        " ".repeat(usage_width.saturating_sub(display_width(&usage)));
                    let text = format!("{marker} {usage}{usage_padding} {description}");
                    let style = if selected {
                        Theme::brand()
                    } else {
                        Theme::dim()
                    };
                    styled_line(text, width.max(1), style, LineFill::Natural)
                })
                .collect();
        }

        if !self.file_palette_visible() {
            return Vec::new();
        }

        let matches = self.file_matches();
        let selected_index = self.file_selection.min(matches.len().saturating_sub(1));
        let (start, above, below) = file_picker::file_palette_scroll_counts(
            matches.len(),
            selected_index,
            MAX_COMMAND_SUGGESTIONS,
        );

        let mut lines = matches
            .iter()
            .enumerate()
            .skip(start)
            .take(MAX_COMMAND_SUGGESTIONS)
            .map(|(index, path)| {
                let selected = index == selected_index;
                let marker = if selected { ">" } else { " " };
                let text = format!("{marker} @{path}");
                let style = if selected {
                    Theme::brand()
                } else {
                    Theme::dim()
                };
                styled_line(
                    truncate_one_line(&text, width.max(1)),
                    width.max(1),
                    style,
                    LineFill::Natural,
                )
            })
            .collect::<Vec<_>>();

        if let Some(footer) = file_picker::file_palette_scroll_footer(above, below, matches.len()) {
            lines.push(styled_line(
                truncate_one_line(&footer, width.max(1)),
                width.max(1),
                Theme::dim(),
                LineFill::Natural,
            ));
        }

        lines
    }

    fn insert_session_intro(&self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let _ = terminal.size()?;
        Ok(())
    }

    fn insert_recovered_history(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let entries = transcript_entries_from_messages(&self.info.recovered_messages);
        if entries.is_empty() {
            return Ok(());
        }

        let width = terminal.size()?.width as usize;
        let (omitted, visible_entries) = recovered_history_tail(
            &entries,
            width,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.max_tool_output_lines,
        );
        let mut transcript = Vec::new();
        if omitted > 0 {
            transcript.push(Entry::Notice(format!(
                "resumed session; showing last {} messages, omitted {omitted} earlier messages",
                visible_entries.len()
            )));
        }
        transcript.extend(visible_entries);
        self.transcript = transcript;
        self.history_lines.invalidate_from(0);
        self.last_status_notice = self.transcript.iter().rev().find_map(|entry| match entry {
            Entry::Notice(text) => Some(text.clone()),
            Entry::User(_)
            | Entry::Assistant(_)
            | Entry::Reasoning(_)
            | Entry::UsageLimits(_)
            | Entry::Tool(_)
            | Entry::Error(_) => None,
        });
        self.last_inserted_was_tool = self.transcript.last().is_some_and(is_tool_entry);
        Ok(())
    }

    fn exit_summary(&self) -> Option<String> {
        self.info
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
            | Entry::UsageLimits(_)
            | Entry::Tool(_)
            | Entry::Error(_) => None,
        };
        self.last_inserted_was_tool = is_tool_entry(&entry);
        self.push_transcript_entry(entry);
    }

    fn push_transcript_entry(&mut self, entry: Entry) {
        match entry {
            Entry::Assistant(text) => match self.transcript.last_mut() {
                Some(Entry::Assistant(previous)) => {
                    previous.push_str(&text);
                    self.history_lines
                        .invalidate_from(self.transcript.len().saturating_sub(1));
                }
                _ => {
                    self.history_lines.invalidate_from(self.transcript.len());
                    self.transcript.push(Entry::Assistant(text));
                }
            },
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

async fn generate_session_title(
    provider_name: String,
    model: String,
    first_user_message: String,
) -> anyhow::Result<String> {
    let provider = build_provider(&provider_name, &model, ReasoningLevel::Low)?;
    let request_messages = vec![
                Message::System(
                    "Generate a concise title for this chat session. Return only the title, no quotes, no punctuation at the end. Use 3 to 7 words."
                        .into(),
                ),
                Message::user_text(format!("First user message:\n\n{first_user_message}")),
            ];
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        provider.send_turn(ModelRequest {
            messages: &request_messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        }),
    )
    .await
    .map_err(|_| anyhow::anyhow!("title generation timed out"))??;
    let ModelResponse::Assistant(blocks) = response;
    let title = blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    sanitize_session_title(&title)
        .ok_or_else(|| anyhow::anyhow!("title model returned an empty title"))
}

fn sanitize_session_title(title: &str) -> Option<String> {
    let title = title
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '*' | '#'))
        .trim()
        .trim_end_matches(['.', ':', ';'])
        .trim();
    if title.is_empty() {
        return None;
    }
    let mut title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > 80 {
        title = title.chars().take(79).collect();
        title.push('…');
    }
    Some(title)
}

fn secret_input_lines(secret: &SecretInput, width: usize) -> Vec<Line<'static>> {
    let masked = "•".repeat(secret.value.chars().count());
    vec![
        styled_line(
            truncate_one_line(
                &format!(
                    "enter API key for {}  enter save, esc cancel",
                    secret.target.provider
                ),
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
        usage.cost_usd_micros = estimated_usage_cost(&usage, metadata);
    }
    usage
}

fn estimated_usage_cost(usage: &ModelUsage, metadata: Option<&ModelMetadata>) -> Option<u64> {
    let metadata = metadata?;
    let input = usage.input_tokens.unwrap_or_default();
    let cache_read = usage.cache_read_tokens.unwrap_or_default();
    let total_input = usage.total_input_tokens().unwrap_or_default();
    let cost = metadata.cost_for_input_tokens(total_input)?;
    let mut micros = 0u128;
    micros += cost_component(input, cost.input_micros_per_m);
    micros += cost_component(
        usage.output_tokens.unwrap_or_default(),
        cost.output_micros_per_m,
    );
    micros += cost_component(cache_read, cost.cache_read_micros_per_m);
    micros += cost_component(
        usage.cache_write_tokens.unwrap_or_default(),
        cost.cache_write_micros_per_m,
    );
    (micros > 0).then_some(micros.min(u64::MAX as u128) as u64)
}

fn cost_component(tokens: u64, micros_per_million: Option<u64>) -> u128 {
    tokens as u128 * micros_per_million.unwrap_or_default() as u128 / 1_000_000
}

fn merge_usage(total: &mut Option<ModelUsage>, usage: ModelUsage) {
    let Some(total) = total.as_mut() else {
        *total = Some(usage);
        return;
    };
    total.input_tokens = add_optional(total.input_tokens, usage.input_tokens);
    total.output_tokens = add_optional(total.output_tokens, usage.output_tokens);
    total.cache_read_tokens = add_optional(total.cache_read_tokens, usage.cache_read_tokens);
    total.cache_write_tokens = add_optional(total.cache_write_tokens, usage.cache_write_tokens);
    total.total_tokens = usage.total_tokens.or_else(|| usage_total_tokens(&usage));
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

fn input_lines_with_images(
    input: &str,
    images: &[ImageContent],
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = images
        .iter()
        .enumerate()
        .map(|(index, image)| {
            styled_line(
                format!("[image {}: {}]", index + 1, image_summary(image)),
                width.max(1),
                Theme::dim(),
                LineFill::Natural,
            )
        })
        .collect::<Vec<_>>();
    lines.extend(input_visual_lines(input, width).into_iter().map(Line::raw));
    lines
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
    use crate::credentials::{
        save_anthropic_api_key, save_openai_api_key, CredentialError, CredentialResult,
        MemoryCredentialStore,
    };
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::{backend::TestBackend, style::Color, Terminal};

    #[path = "layout_tests.rs"]
    mod layout_tests;
    #[path = "mouse_tests.rs"]
    mod mouse_tests;
    #[path = "questionnaire_interaction_tests.rs"]
    mod questionnaire_interaction_tests;

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
        })
    }

    pub(super) fn test_app() -> App {
        let store = Arc::new(MemoryCredentialStore::default());
        save_openai_api_key(store.as_ref(), "sk-test").unwrap();
        App::new_with_credentials(
            TuiInfo {
                cwd: PathBuf::from("/tmp/project"),
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                reasoning: ReasoningLevel::Low,
                show_reasoning_output: true,
                auth: "api-key".into(),
                title_provider: None,
                title_model: None,
                title_auth: None,
                favorite_models: Vec::new(),
                questionnaire_enabled: true,
                session_id: None,
                recovered_messages: Vec::new(),
                open_resume_picker: false,
                config_repository: ConfigRepository::new(None),
                auth_unavailable: None,
                update_notice: None,
                pending_update_notice: None,
                diagnostics: crate::diagnostics::test_diagnostics("openai", "gpt-test"),
                herdr: HerdrReporter::default(),
                max_tool_output_lines: 10,
                keybindings: Keybindings::default(),
                prompt_templates: Default::default(),
            },
            store,
        )
    }

    #[derive(Debug)]
    struct ReasoningRecordingProvider {
        levels: Arc<Mutex<Vec<ReasoningLevel>>>,
    }

    #[async_trait::async_trait(?Send)]
    impl crate::model::ModelProvider for ReasoningRecordingProvider {
        fn set_reasoning(&mut self, reasoning: ReasoningLevel) -> bool {
            self.levels.lock().unwrap().push(reasoning);
            true
        }

        async fn send_turn(
            &self,
            _request: ModelRequest<'_>,
        ) -> Result<ModelResponse, crate::model::ModelError> {
            unreachable!("metadata tests do not send model requests")
        }
    }

    #[tokio::test]
    async fn metadata_fetch_completion_normalizes_active_provider_reasoning() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let config = crate::config::Config {
            reasoning: ReasoningLevel::Max,
            ..crate::config::Config::default()
        };
        config.save(Some(config_path.clone())).unwrap();

        let mut app = test_app();
        app.info.reasoning = ReasoningLevel::Max;
        app.info.config_repository = ConfigRepository::new(Some(config_path.clone()));
        app.pending_model_metadata = Some(tokio::spawn(async {
            Some(ModelMetadata {
                supported_reasoning_levels: Some(vec![
                    ReasoningLevel::Off,
                    ReasoningLevel::Low,
                    ReasoningLevel::High,
                ]),
                ..ModelMetadata::default()
            })
        }));
        let recorded_levels = Arc::new(Mutex::new(Vec::new()));
        let provider = ReasoningRecordingProvider {
            levels: Arc::clone(&recorded_levels),
        };
        let mut agent = Agent::new(
            Box::new(provider),
            crate::tool::ToolRegistry::new(),
            crate::tool::ToolContext {
                cwd: temp_dir.path().into(),
                max_output_bytes: 12_000,
            },
        );
        tokio::task::yield_now().await;

        app.poll_model_metadata_fetch(&mut agent);

        assert_eq!(app.info.reasoning, ReasoningLevel::High);
        assert_eq!(*recorded_levels.lock().unwrap(), vec![ReasoningLevel::High]);
        assert_eq!(
            crate::config::Config::load(Some(config_path))
                .unwrap()
                .reasoning,
            ReasoningLevel::High
        );
    }

    #[test]
    fn info_command_uses_runtime_diagnostics() {
        let mut app = test_app();

        app.execute_info_command().unwrap();

        assert!(matches!(
            app.transcript.last(),
            Some(Entry::Notice(message))
                if message.contains("rho ")
                    && message.contains("provider: openai")
                    && message.contains("model: gpt-test")
                    && message.contains("reasoning: medium")
        ));
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
            sanitize_session_title("\"Implement resume picker.\""),
            Some("Implement resume picker".into())
        );
        assert_eq!(sanitize_session_title("\n\n"), None);
    }

    #[test]
    fn title_model_defaults_to_main_model() {
        let app = test_app();

        assert_eq!(
            app.title_model_selection(),
            ("openai".into(), "gpt-5.5".into(), "api-key".into())
        );
    }

    #[test]
    fn estimated_usage_cost_uses_normalized_input_and_cache_read() {
        let metadata = ModelMetadata {
            cost_default: Some(crate::model::models_dev::ModelCost {
                input_micros_per_m: Some(1_000_000),
                output_micros_per_m: Some(2_000_000),
                cache_read_micros_per_m: Some(100_000),
                cache_write_micros_per_m: Some(500_000),
            }),
            ..ModelMetadata::default()
        };
        let usage = ModelUsage {
            input_tokens: Some(300_000),
            cache_read_tokens: Some(700_000),
            output_tokens: Some(100_000),
            cache_write_tokens: Some(10_000),
            ..ModelUsage::default()
        };

        assert_eq!(estimated_usage_cost(&usage, Some(&metadata)), Some(575_000));
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
            .record_agent_event(AgentEvent::ContextUsage(ContextUsage::estimated(
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
    fn usage_event_tracks_latest_usage_separately_from_cumulative_totals() {
        let mut app = test_app();

        assert!(app
            .record_agent_event(AgentEvent::Usage(ModelUsage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_tokens: Some(400),
                ..ModelUsage::default()
            }))
            .is_none());
        assert!(app
            .record_agent_event(AgentEvent::Usage(ModelUsage {
                input_tokens: Some(300),
                output_tokens: Some(40),
                cache_read_tokens: Some(700),
                ..ModelUsage::default()
            }))
            .is_none());

        assert_eq!(
            app.latest_usage,
            Some(ModelUsage {
                input_tokens: Some(300),
                output_tokens: Some(40),
                cache_read_tokens: Some(700),
                ..ModelUsage::default()
            })
        );
        assert_eq!(
            app.cumulative_usage,
            Some(ModelUsage {
                input_tokens: Some(400),
                output_tokens: Some(60),
                cache_read_tokens: Some(1_100),
                total_tokens: Some(1_040),
                ..ModelUsage::default()
            })
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

        assert!(app.recall_last_queued_prompt());
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

        assert!(app.recall_last_queued_prompt());
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
    fn paste_segment_is_removed_when_marker_is_edited() {
        let mut app = test_app();
        app.insert_pasted_input_text("alpha\nbeta");
        app.input_cursor = 1;
        app.delete_input();

        assert_eq!(app.input, "[pasted: 2 lines ]");
        assert!(app.paste_segments.is_empty());
        assert_eq!(app.expanded_input(), "[pasted: 2 lines ]");
    }

    #[test]
    fn normalize_paste_converts_carriage_returns() {
        assert_eq!(normalize_paste("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn recovered_session_messages_become_transcript_entries() {
        let entries = transcript_entries_from_messages(&[
            Message::System("system".into()),
            Message::User(vec![
                ContentBlock::Text("hello".into()),
                ContentBlock::Image(ImageContent {
                    data: "aW1n".into(),
                    mime_type: "image/png".into(),
                }),
            ]),
            Message::Assistant(vec![ContentBlock::Text("hi".into())]),
            Message::Assistant(vec![ContentBlock::ToolCall(crate::tool::ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            })]),
            Message::ToolResult(crate::tool::ToolResult {
                id: "call_1".into(),
                ok: false,
                content: "missing file".into(),
            }),
        ]);

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
            }) if display_lines == &vec!["read_file".to_string(), "missing file".to_string()]
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
        app.info.max_tool_output_lines = 1;
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
            .rposition(|entry| expandable_tool_entry(entry, app.info.max_tool_output_lines))
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

        assert!(app.record_agent_event(AgentEvent::StepStarted(2)).is_none());

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

        assert!(rendered.contains("working"), "{rendered}");
        assert!(!rendered.contains("hello"), "{rendered}");
        assert!(!rendered.contains("thinking"), "{rendered}");
    }

    #[test]
    fn input_divider_style_tracks_reasoning_level() {
        let mut app = test_app();
        app.input = "hello".into();

        app.info.reasoning = ReasoningLevel::Off;
        let off_lines = app.active_lines(20);
        let off_divider = off_lines
            .iter()
            .find(|line| line_text(line) == "────────────────────")
            .unwrap();
        let off_style = off_divider.style;

        app.info.reasoning = ReasoningLevel::High;
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

        assert!(!small_rendered.contains("working"), "{small_rendered}");
        assert!(default_rendered.contains("working"), "{default_rendered}");
    }

    #[test]
    fn spinner_is_anchored_immediately_above_composer_divider() {
        let mut app = test_app();
        app.running = true;
        app.pending_tool_call = Some(ToolEntry {
            state: ToolEntryState::Running,
            display_lines: vec!["bash".into(), "cargo test".into()],
            expanded: false,
        });
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
        assert!(rows[activity.y as usize].contains("working"), "{rows:#?}");
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

        assert!(!rendered.contains("working"), "{rendered}");
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
        app.info.cwd = workspace.path().to_path_buf();
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
        app.info.cwd = workspace.path().to_path_buf();
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

        assert!(rendered.contains("enter API key for openai"), "{rendered}");
        assert!(rendered.contains("••••"), "{rendered}");
        assert!(!rendered.contains("sk-secret-value"), "{rendered}");
    }

    #[test]
    fn login_provider_picker_uses_provider_names_only() {
        let mut app = test_app();
        app.open_provider_picker("login", PickerAction::LoginProvider);

        let rendered = app
            .active_lines(80)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("openai"), "{rendered}");
        assert!(rendered.contains("openai-codex"), "{rendered}");
        assert!(rendered.contains("anthropic"), "{rendered}");
        assert!(!rendered.contains("api-key"), "{rendered}");
        assert!(!rendered.contains("> codex"), "{rendered}");
    }

    #[test]
    fn logout_provider_picker_uses_only_providers_with_stored_credentials() {
        let store = MemoryCredentialStore::default();
        save_openai_api_key(&store, "sk-test").unwrap();
        save_anthropic_api_key(&store, "sk-ant-test").unwrap();

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
        save_openai_api_key(store.as_ref(), "sk-test").unwrap();
        save_codex_tokens(
            store.as_ref(),
            &crate::credentials::CodexTokens {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                id_token: None,
                account_id: None,
            },
        )
        .unwrap();
        save_anthropic_api_key(store.as_ref(), "sk-ant-test").unwrap();
        let mut app = App::new_with_credentials(
            TuiInfo {
                cwd: PathBuf::from("/tmp/project"),
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                reasoning: ReasoningLevel::Low,
                show_reasoning_output: true,
                auth: "api-key".into(),
                title_provider: None,
                title_model: None,
                title_auth: None,
                favorite_models: Vec::new(),
                questionnaire_enabled: true,
                session_id: None,
                recovered_messages: Vec::new(),
                open_resume_picker: false,
                config_repository: ConfigRepository::new(None),
                auth_unavailable: None,
                update_notice: None,
                pending_update_notice: None,
                diagnostics: crate::diagnostics::test_diagnostics("openai", "gpt-test"),
                herdr: HerdrReporter::default(),
                max_tool_output_lines: 10,
                keybindings: Keybindings::default(),
                prompt_templates: Default::default(),
            },
            store,
        );
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
        app.info.config_repository = ConfigRepository::new(Some(config_dir.path().to_path_buf()));
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
        assert!(app.info.favorite_models.is_empty());
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
        app.info.config_repository =
            ConfigRepository::new(Some(config_dir.path().join("config.toml")));
        let config = app.info.config_repository.load().unwrap();
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
    fn esc_from_nested_web_search_config_returns_to_main_config() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut app = test_app();
        app.info.config_repository =
            ConfigRepository::new(Some(config_dir.path().join("config.toml")));
        let config = app.info.config_repository.load().unwrap();
        app.composer = ComposerMode::Picker(config_picker::web_search_config_picker(
            &config,
            app.credential_store.as_ref(),
        ));

        app.handle_picker_escape(/*running*/ false).unwrap();

        let ComposerMode::Picker(picker) = &app.composer else {
            panic!("expected picker after nested config escape");
        };
        assert_eq!(
            picker.selected_item().unwrap().value,
            config_picker::WEB_SEARCH_VALUE
        );
        assert!(!app.web_search_config_picker_is_open());
        assert_eq!(app.status, "config");
    }

    #[test]
    fn esc_from_main_config_still_closes_picker() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut app = test_app();
        app.info.config_repository =
            ConfigRepository::new(Some(config_dir.path().join("config.toml")));
        let config = app.info.config_repository.load().unwrap();
        app.composer = ComposerMode::Picker(config_picker::config_picker(&app.info, &config));

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
    fn alt_up_recalls_last_queued_message_for_editing() {
        let mut app = test_app();
        app.queued_prompts.push_back("first queued".into());
        app.queued_prompts.push_back("second queued".into());

        assert!(app.recall_last_queued_prompt());

        assert_eq!(app.input, "second queued");
        assert_eq!(app.input_cursor, "second queued".chars().count());
        assert_eq!(app.queued_prompts, VecDeque::from(["first queued".into()]));
    }

    #[test]
    fn alt_up_removed_queued_messages_do_not_enter_prompt_history() {
        let mut app = test_app();
        app.queued_prompts.push_back("first queued".into());
        app.queued_prompts.push_back("second queued".into());

        assert!(app.recall_last_queued_prompt());
        app.input.clear();
        app.input_cursor = 0;
        assert!(app.recall_last_queued_prompt());

        assert_eq!(app.input, "first queued");
        assert!(app.queued_prompts.is_empty());
        assert!(!app.recall_input_history(HistoryDirection::Previous));
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
        app.info.cwd = project.path().to_path_buf();
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
        app.pending_tool_call = Some(ToolEntry {
            state: ToolEntryState::Running,
            display_lines: vec!["bash".into(), "cargo test".into()],
            expanded: false,
        });
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
        assert!(!rendered.contains("working"), "{rendered}");
    }

    #[test]
    fn exit_summary_is_minimal_and_session_only() {
        let mut app = test_app();
        assert_eq!(app.exit_summary(), None);

        app.info.session_id = Some("session-123".into());
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
