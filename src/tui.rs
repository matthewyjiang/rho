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
use tokio::sync::mpsc;

use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
    DefaultTerminal, Frame, Terminal, TerminalOptions, Viewport,
};
mod config_picker;
mod login;
mod markdown;
mod model_picker;
mod picker;
mod provider_picker;
mod render;
mod session_picker;
mod skill_picker;
mod statusline;
mod stream;
mod theme;

use markdown::push_wrapped_markdown;
use picker::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, UiPicker};
use render::{
    entry_lines, input_cursor_position, input_visual_lines, picker_lines, push_wrapped_text,
    session_header_lines, styled_line, truncate_one_line, LineFill,
};
use statusline::{statusline_lines, StatusLineState};
use stream::{AppendOnlyStream, StreamFragment};
use theme::Theme;

use crate::{
    agent::{Agent, AgentEvent, SessionHistorySink},
    auth::{codex_oauth, github_copilot_device},
    clipboard_image::read_clipboard_image,
    commands::{self, CommandId, CommandInvocation, CommandSpec},
    config::Config,
    credentials::{
        available_auth_modes, delete_provider_credentials, provider_has_credentials,
        provider_has_env_override, save_codex_tokens, save_github_copilot_tokens,
        save_provider_api_key, CodexTokens, CredentialStore, GitHubCopilotTokens,
        OsCredentialStore,
    },
    model::{
        build_provider,
        catalog::{self, LoginTarget, ModelSelection},
        image_summary,
        models_dev::{cached_model_metadata, fetch_model_metadata},
        provider_models::refresh_provider_models_with_store,
        registry::{self, ProviderAuthKind},
        ContentBlock, ContextUsage, ImageContent, Message, ModelMetadata, ModelRequest,
        ModelResponse, ModelUsage, UnavailableProvider,
    },
    reasoning::ReasoningLevel,
    session::Session,
    tool::ToolDisplayStyle,
};

const INLINE_VIEWPORT_HEIGHT: u16 = 18;
const PASTE_BURST_GAP: Duration = Duration::from_millis(12);
const PASTE_ENTER_SUPPRESSION: Duration = Duration::from_millis(120);
const PASTE_BURST_MIN_CHARS: usize = 2;
const PASTE_COLLAPSE_MIN_LINES: usize = 2;
const PASTE_COLLAPSE_MIN_CHARS: usize = 1000;
const MAX_COMMAND_SUGGESTIONS: usize = 5;
const RECOVERED_HISTORY_LINE_LIMIT: usize = 200;

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
    pub max_tool_output_lines: usize,
    pub session_id: Option<String>,
    pub open_resume_picker: bool,
    pub config_path: Option<PathBuf>,
    pub auth_unavailable: Option<String>,
    pub update_notice: Option<String>,
}

pub struct TuiResult {
    pub resume_session_id: Option<String>,
    exit_lines: Vec<Line<'static>>,
}

pub async fn run(agent: &mut Agent, info: TuiInfo) -> anyhow::Result<TuiResult> {
    agent.set_session_id(info.session_id.clone());
    let mut terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(INLINE_VIEWPORT_HEIGHT),
    });
    Theme::initialize_from_terminal();
    let bracketed_paste_enabled = enable_bracketed_paste().is_ok();
    let modified_keys_enabled = enable_modified_keys().is_ok();
    let keyboard_enhancements_enabled = enable_keyboard_enhancements().is_ok();
    let result = App::new(info).run(&mut terminal, agent).await;
    if keyboard_enhancements_enabled {
        let _ = disable_keyboard_enhancements();
    }
    if modified_keys_enabled {
        let _ = disable_modified_keys();
    }
    if bracketed_paste_enabled {
        let _ = disable_bracketed_paste();
    }
    ratatui::restore();
    if let Ok(result) = &result {
        print_exit_lines(&result.exit_lines)?;
    }
    result
}

struct App {
    info: TuiInfo,
    input: String,
    input_cursor: usize,
    status: String,
    should_quit: bool,
    ctrl_c_streak: u8,
    assistant_stream: AppendOnlyStream,
    assistant_stream_in_code_block: bool,
    reasoning_stream: AppendOnlyStream,
    current_stream_kind: Option<StreamKind>,
    current_turn_start: Option<usize>,
    active_turn_show_reasoning_output: bool,
    running: bool,
    loading_spinner: LoadingSpinner,
    active_tool_call: bool,
    pending_tool_call: Option<ToolEntry>,
    steering_prompts: VecDeque<String>,
    queued_prompts: VecDeque<QueuedPrompt>,
    pending_images: Vec<ImageContent>,
    input_history: Vec<String>,
    input_history_cursor: Option<usize>,
    input_history_draft: Option<InputDraft>,
    paste_burst: PasteBurst,
    paste_segments: Vec<PasteSegment>,
    transcript: Vec<Entry>,
    last_inserted_was_tool: bool,
    command_selection: usize,
    command_prefix: Option<String>,
    command_palette_dismissed: bool,
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
    pending_session_title: Option<Pin<Box<dyn Future<Output = SessionTitleResult>>>>,
    inline_viewport_height: u16,
}

#[derive(Clone, Debug)]
enum ComposerMode {
    Input,
    Picker(UiPicker),
    SecretInput(SecretInput),
    ConfigNumberInput(ConfigNumberInput),
    ConfigTextInput(ConfigTextInput),
    OAuthPending(LoginTarget),
}

#[derive(Clone, Debug)]
struct ConfigNumberInput {
    key: ConfigNumberKey,
    value: String,
    cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigNumberKey {
    MaxOutputBytes,
    MaxToolOutputLines,
}

#[derive(Clone, Debug)]
struct ConfigTextInput {
    key: ConfigTextKey,
    value: String,
    cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigTextKey {
    OpenAiSearch,
    Exa,
    Brave,
}

impl ConfigNumberKey {
    fn label(self) -> &'static str {
        match self {
            ConfigNumberKey::MaxOutputBytes => "max output bytes",
            ConfigNumberKey::MaxToolOutputLines => "max tool output lines",
        }
    }
}

impl ConfigTextKey {
    fn label(self) -> &'static str {
        match self {
            ConfigTextKey::OpenAiSearch => "OpenAI web search API key",
            ConfigTextKey::Exa => "Exa API key",
            ConfigTextKey::Brave => "Brave Search API key",
        }
    }

    fn picker_value(self) -> &'static str {
        match self {
            ConfigTextKey::OpenAiSearch => config_picker::WEB_SEARCH_OPENAI_KEY_VALUE,
            ConfigTextKey::Exa => config_picker::WEB_SEARCH_EXA_KEY_VALUE,
            ConfigTextKey::Brave => config_picker::WEB_SEARCH_BRAVE_KEY_VALUE,
        }
    }
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

#[derive(Clone, Debug, Default)]
struct LoadingSpinner {
    started_at: Option<Instant>,
}

impl LoadingSpinner {
    const FRAMES: [&'static str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const FRAME_INTERVAL: Duration = Duration::from_millis(80);

    fn start(&mut self) {
        self.started_at = Some(Instant::now());
    }

    fn start_if_needed(&mut self) {
        if self.started_at.is_none() {
            self.start();
        }
    }

    fn stop(&mut self) {
        self.started_at = None;
    }

    fn frame_at(&self, now: Instant) -> &'static str {
        let Some(started_at) = self.started_at else {
            return Self::FRAMES[0];
        };
        let interval_ms = Self::FRAME_INTERVAL.as_millis().max(1);
        let frame = now
            .saturating_duration_since(started_at)
            .as_millis()
            .checked_div(interval_ms)
            .unwrap_or(0) as usize;
        Self::FRAMES[frame % Self::FRAMES.len()]
    }

    fn line(&self, now: Instant) -> Line<'static> {
        Line::from(vec![
            Span::styled(self.frame_at(now), Theme::accent()),
            Span::styled(" working", Theme::dim()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CommandChoiceKind {
    Builtin(&'static CommandSpec),
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
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamKind {
    Assistant,
    Reasoning,
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

#[derive(Default)]
struct PasteBurst {
    last_plain_char_at: Option<Instant>,
    plain_char_count: usize,
    suppress_enter_until: Option<Instant>,
}

impl ConfigNumberInput {
    fn new(key: ConfigNumberKey, value: usize) -> Self {
        let value = value.to_string();
        let cursor = value.chars().count();
        Self { key, value, cursor }
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    fn insert_char(&mut self, ch: char) {
        if !ch.is_ascii_digit() {
            return;
        }
        let byte_index = self.byte_index(self.cursor);
        self.value.insert(byte_index, ch);
        self.cursor += 1;
    }

    fn insert_text(&mut self, text: &str) {
        for ch in text.chars().filter(|ch| ch.is_ascii_digit()) {
            self.insert_char(ch);
        }
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
}

impl ConfigTextInput {
    fn new(key: ConfigTextKey, value: Option<String>) -> Self {
        let value = value.unwrap_or_default();
        let cursor = value.chars().count();
        Self { key, value, cursor }
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
        if ch == '\n' || ch == '\r' {
            return;
        }
        let byte_index = self.byte_index(self.cursor);
        self.value.insert(byte_index, ch);
        self.cursor += 1;
    }

    fn insert_text(&mut self, text: &str) {
        let sanitized = text.replace(['\n', '\r'], "");
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

impl PasteBurst {
    fn record_plain_char(&mut self, now: Instant) {
        self.plain_char_count = match self.last_plain_char_at {
            Some(last) if now.saturating_duration_since(last) <= PASTE_BURST_GAP => {
                self.plain_char_count.saturating_add(1)
            }
            _ => 1,
        };
        self.last_plain_char_at = Some(now);
        if self.plain_char_count >= PASTE_BURST_MIN_CHARS {
            self.suppress_enter_until = now.checked_add(PASTE_ENTER_SUPPRESSION);
        }
    }

    fn should_insert_newline_for_enter(&mut self, now: Instant) -> bool {
        if self
            .suppress_enter_until
            .is_some_and(|deadline| now <= deadline)
        {
            self.suppress_enter_until = now.checked_add(PASTE_ENTER_SUPPRESSION);
            true
        } else {
            self.clear();
            false
        }
    }

    fn clear(&mut self) {
        self.last_plain_char_at = None;
        self.plain_char_count = 0;
        self.suppress_enter_until = None;
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
        Self {
            info,
            input: String::new(),
            input_cursor: 0,
            status,
            should_quit: false,
            ctrl_c_streak: 0,
            assistant_stream: AppendOnlyStream::default(),
            assistant_stream_in_code_block: false,
            reasoning_stream: AppendOnlyStream::default(),
            current_stream_kind: None,
            current_turn_start: None,
            active_turn_show_reasoning_output,
            running: false,
            loading_spinner: LoadingSpinner::default(),
            active_tool_call: false,
            pending_tool_call: None,
            steering_prompts: VecDeque::new(),
            queued_prompts: VecDeque::new(),
            pending_images: Vec::new(),
            input_history: Vec::new(),
            input_history_cursor: None,
            input_history_draft: None,
            paste_burst: PasteBurst::default(),
            paste_segments: Vec::new(),
            transcript: Vec::new(),
            last_inserted_was_tool: false,
            command_selection: 0,
            command_prefix: None,
            command_palette_dismissed: false,
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
            pending_session_title: None,
            inline_viewport_height: INLINE_VIEWPORT_HEIGHT,
        }
    }

    async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<TuiResult> {
        self.start_model_metadata_fetch(agent);
        self.insert_session_intro(terminal)?;
        self.insert_recovered_history(terminal, agent)?;
        if self.info.open_resume_picker {
            self.open_resume_picker(terminal)?;
        }
        if self.info.auth_unavailable.is_some() {
            self.insert_entry(
                terminal,
                &Entry::Notice("no providers configured. run /login to sign in.".into()),
            )?;
        }
        while !self.should_quit {
            self.poll_model_metadata_fetch(agent);
            self.poll_pending_session_title(terminal)?;
            self.poll_pending_oauth_login(terminal, agent).await?;
            self.resize_inline_viewport_if_needed(terminal)?;
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.handle_key(key, terminal, agent).await?;
                    }
                    Event::Paste(text) => {
                        let text = normalize_paste(&text);
                        match &mut self.composer {
                            ComposerMode::Input => self.insert_pasted_input_text(&text),
                            ComposerMode::SecretInput(secret) => secret.insert_text(&text),
                            ComposerMode::ConfigNumberInput(input) => input.insert_text(&text),
                            ComposerMode::ConfigTextInput(input) => input.insert_text(&text),
                            ComposerMode::Picker(_) | ComposerMode::OAuthPending(_) => {}
                        }
                        self.paste_burst.clear();
                    }
                    Event::Resize(_, _) => {
                        self.reflow_history(terminal)?;
                    }
                    _ => {}
                }
            }
        }
        let width = terminal.size()?.width as usize;
        let exit_lines = self.exit_lines(width);
        Ok(TuiResult {
            resume_session_id: self.info.session_id,
            exit_lines,
        })
    }

    async fn handle_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        if self.handle_oauth_pending_key(key, terminal)? {
            return Ok(());
        }

        if self.handle_secret_key(key, terminal, agent).await? {
            return Ok(());
        }

        if self.handle_config_number_key(key, terminal)? {
            return Ok(());
        }

        if self.handle_config_text_key(key, terminal)? {
            return Ok(());
        }

        if self.handle_reasoning_cycle_key(key, terminal, agent)? {
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

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    self.input.clear();
                    self.paste_segments.clear();
                    self.pending_images.clear();
                    self.input_cursor = 0;
                    self.clamp_command_selection();
                    self.status = "input cleared; press ctrl-c again to quit".into();
                    self.ctrl_c_streak = 1;
                } else {
                    self.should_quit = true;
                }
            }
            (_, KeyCode::Esc) => {
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('v'))
            | (KeyModifiers::ALT, KeyCode::Char('v')) => {
                self.paste_clipboard_image();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('o')) => {
                self.toggle_latest_tool_output(terminal)?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                agent.reset();
                self.info.session_id = None;
                agent.set_session_id(None);
                agent.clear_history_sink();
                self.cumulative_usage = None;
                self.latest_usage = None;
                self.current_context = None;
                self.insert_entry(
                    terminal,
                    &Entry::Notice("conversation reset; next message starts a new session".into()),
                )?;
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
                    self.status = format!(
                        "editing queued message; {} queued message(s) remain",
                        self.queued_prompts.len()
                    );
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
            (KeyModifiers::CONTROL, KeyCode::Char('j')) => {
                self.insert_input_char('\n');
                self.paste_burst.clear();
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
                if self
                    .paste_burst
                    .should_insert_newline_for_enter(Instant::now())
                {
                    self.insert_input_char('\n');
                } else {
                    self.submit(terminal, agent).await?;
                }
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_input_char(ch);
                self.paste_burst.record_plain_char(Instant::now());
                self.ctrl_c_streak = 0;
            }
            _ => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
        }
        self.clamp_command_selection();
        Ok(())
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
                self.model_metadata = Some(metadata);
            }
        }
    }

    fn poll_pending_session_title(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let Some(future) = self.pending_session_title.as_mut() else {
            return Ok(());
        };
        let waker = noop_waker_ref();
        let mut context = std::task::Context::from_waker(waker);
        let std::task::Poll::Ready(result) = future.as_mut().poll(&mut context) else {
            return Ok(());
        };
        self.pending_session_title = None;
        let Ok(title) = result.title else {
            return Ok(());
        };
        if Session::set_title(&self.info.cwd, &result.session_id, &title).is_err() {
            return Ok(());
        }
        if self.info.session_id.as_deref() == Some(result.session_id.as_str()) {
            self.insert_entry(terminal, &Entry::Notice(format!("session titled: {title}")))?;
        }
        Ok(())
    }

    fn handle_oauth_pending_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
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
                self.insert_entry(
                    terminal,
                    &Entry::Notice(format!("{provider} login cancelled")),
                )?;
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
                self.paste_burst.record_plain_char(Instant::now());
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
                let Ok(mut value) = input.value.parse::<usize>() else {
                    self.insert_entry(
                        terminal,
                        &Entry::Error(format!(
                            "{} must be a positive whole number",
                            input.key.label()
                        )),
                    )?;
                    self.status = "config save failed".into();
                    return Ok(true);
                };
                value = value.max(1);
                match input.key {
                    ConfigNumberKey::MaxOutputBytes => {
                        Config::load(self.info.config_path.clone()).and_then(|mut config| {
                            config.max_output_bytes = value;
                            config.save(self.info.config_path.clone())
                        })?;
                        self.composer = ComposerMode::Picker(config_picker::config_picker(
                            &self.info,
                            value,
                            self.info.max_tool_output_lines,
                        ));
                        self.insert_entry(
                            terminal,
                            &Entry::Notice(format!(
                                "max output bytes set to {value}; applies next session"
                            )),
                        )?;
                        self.status = "config saved".into();
                    }
                    ConfigNumberKey::MaxToolOutputLines => {
                        Config::load(self.info.config_path.clone()).and_then(|mut config| {
                            config.max_tool_output_lines = value;
                            config.save(self.info.config_path.clone())
                        })?;
                        self.info.max_tool_output_lines = value;
                        self.composer = ComposerMode::Picker(config_picker::config_picker(
                            &self.info,
                            Config::load(self.info.config_path.clone())?.max_output_bytes,
                            value,
                        ));
                        self.reflow_history(terminal)?;
                        self.insert_entry(
                            terminal,
                            &Entry::Notice(format!("max tool output lines set to {value}")),
                        )?;
                        self.status = "config saved".into();
                    }
                }
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
            (_, KeyCode::Esc) => {
                let config = Config::load(self.info.config_path.clone())?;
                self.info.show_reasoning_output = config.show_reasoning_output;
                self.composer = ComposerMode::Picker(config_picker::config_picker(
                    &self.info,
                    config.max_output_bytes,
                    config.max_tool_output_lines,
                ));
                self.status = "config".into();
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn handle_config_text_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigTextInput(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let ComposerMode::ConfigTextInput(input) = &self.composer else {
                    return Ok(true);
                };
                let key = input.key;
                let value = input.value.trim().to_string();
                let save_result =
                    Config::load(self.info.config_path.clone()).and_then(|mut config| {
                        let value = (!value.is_empty()).then_some(value);
                        match key {
                            ConfigTextKey::OpenAiSearch => config.web_search_openai_api_key = value,
                            ConfigTextKey::Exa => config.web_search_exa_api_key = value,
                            ConfigTextKey::Brave => config.web_search_brave_api_key = value,
                        }
                        config.save(self.info.config_path.clone())
                    });
                match save_result {
                    Ok(()) => {
                        self.refresh_web_search_config_picker(key.picker_value());
                        self.status = format!("{} saved", key.label());
                    }
                    Err(err) => {
                        self.insert_entry(
                            terminal,
                            &Entry::Error(format!("could not save {}: {err}", key.label())),
                        )?;
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
            (_, KeyCode::Esc) => {
                let ComposerMode::ConfigTextInput(input) = &self.composer else {
                    return Ok(true);
                };
                self.refresh_web_search_config_picker(input.key.picker_value());
                self.status = "web search config".into();
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn handle_reasoning_cycle_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<bool> {
        let is_shift_tab = matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT));
        if !is_shift_tab {
            return Ok(false);
        }

        self.cycle_reasoning(terminal, agent)?;
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
                    let (input, cursor) = self.complete_command_choice(&choice);
                    self.input = input;
                    self.input_cursor = cursor;
                    self.command_palette_dismissed = false;
                    self.clamp_command_selection();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(choice) = self.selected_command() {
                    let (input, cursor) = self.complete_command_choice(&choice);
                    self.input = input;
                    self.input_cursor = cursor;
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
                });
                self.input = draft.input;
                self.paste_segments = draft.paste_segments;
                self.input_cursor = self.input_char_len();
                self.input_history_cursor = None;
                self.input_changed();
                return true;
            }
        };

        self.input = self.input_history[next_cursor].clone();
        self.paste_segments.clear();
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

        let width = terminal_width.max(1);
        match direction {
            HistoryDirection::Previous => {
                self.input_cursor = self.input_cursor.saturating_sub(width);
            }
            HistoryDirection::Next => {
                self.input_cursor = (self.input_cursor + width).min(self.input_char_len());
            }
        }
    }

    fn recall_last_queued_prompt(&mut self) -> bool {
        let Some(prompt) = self.queued_prompts.pop_back() else {
            return false;
        };
        self.input = prompt.display_prompt;
        self.paste_segments = prompt.paste_segments;
        self.input_cursor = self.input_char_len();
        self.reset_input_history_navigation();
        self.input_changed();
        true
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
        self.clamp_command_selection();
    }

    fn command_matches(&self) -> Vec<CommandChoice> {
        let Some(prefix) = commands::command_prefix(&self.input) else {
            return Vec::new();
        };
        let prefix = prefix
            .strip_prefix('/')
            .unwrap_or(prefix)
            .to_ascii_lowercase();
        let mut matches = commands::matching_commands(&prefix)
            .into_iter()
            .map(|command| CommandChoice {
                name: command.name.to_string(),
                usage: command.usage.to_string(),
                description: command.description.to_string(),
                kind: CommandChoiceKind::Builtin(command),
            })
            .collect::<Vec<_>>();
        matches.extend(
            crate::skills::discover(&self.info.cwd)
                .into_iter()
                .filter(|skill| {
                    skill.name.starts_with(&prefix)
                        || format!("skill:{}", skill.name).starts_with(&prefix)
                })
                .map(|skill| {
                    let command_name = format!("skill:{}", skill.name);
                    CommandChoice {
                        usage: format!("/{command_name}"),
                        name: command_name,
                        description: skill.description,
                        kind: CommandChoiceKind::Skill,
                    }
                }),
        );
        matches
    }

    fn selected_command(&self) -> Option<CommandChoice> {
        let matches = self.command_matches();
        matches
            .get(self.command_selection.min(matches.len().saturating_sub(1)))
            .cloned()
    }

    fn complete_command_choice(&self, choice: &CommandChoice) -> (String, usize) {
        match &choice.kind {
            CommandChoiceKind::Builtin(spec) => {
                commands::complete_command(&self.input, self.input_cursor, spec)
            }
            CommandChoiceKind::Skill => {
                complete_slash_command(&self.input, self.input_cursor, &choice.name)
            }
        }
    }

    fn command_palette_visible(&self) -> bool {
        !self.command_palette_dismissed
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
            self.input.clear();
            self.paste_segments.clear();
            self.input_cursor = 0;
            self.clamp_command_selection();
            return Ok(());
        }

        match commands::parse_command(&self.input) {
            Ok(Some(invocation)) => {
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
                if self.execute_skill_command(&name, terminal, agent)? {
                    if trailing_prompt.is_empty() {
                        return Ok(());
                    }
                    prompt = trailing_prompt;
                    display_prompt = trailing_display_prompt;
                } else {
                    self.insert_entry(
                        terminal,
                        &Entry::Error(format!(
                            "unknown command '/{name}'. Type / to choose one of: {}",
                            commands::COMMANDS
                                .iter()
                                .map(|command| command.usage)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                    )?;
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
        self.run_prompt_turn(prompt, display_prompt, images, terminal, agent)
            .await?;
        while !self.should_quit {
            let Some(prompt) = self.queued_prompts.pop_front() else {
                break;
            };
            self.run_prompt_turn(
                prompt.prompt,
                prompt.display_prompt,
                Vec::new(),
                terminal,
                agent,
            )
            .await?;
        }
        Ok(())
    }

    async fn run_prompt_turn(
        &mut self,
        prompt: String,
        display_prompt: String,
        images: Vec<ImageContent>,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        if !prompt.is_empty() {
            self.push_input_history(&prompt);
        }
        self.reset_input_history_navigation();
        self.ensure_session(agent)?;
        if !agent
            .messages()
            .iter()
            .any(|message| matches!(message, Message::User(_)))
        {
            self.start_session_title_generation(prompt.clone());
        }
        self.insert_entry(
            terminal,
            &Entry::User(render_user_entry(&display_prompt, &images)),
        )?;
        self.current_turn_start = Some(self.transcript.len());
        self.active_turn_show_reasoning_output = self.info.show_reasoning_output;
        self.reset_streams();
        self.status = "running".into();
        self.running = true;
        self.loading_spinner.start();
        self.resize_inline_viewport_if_needed(terminal)?;
        terminal.draw(|frame| self.draw(frame))?;

        self.active_tool_call = false;
        self.pending_tool_call = None;
        let interrupt_requested = Arc::new(AtomicBool::new(false));
        let tool_call_active = Arc::new(AtomicBool::new(false));
        let steering_prompts = Arc::new(Mutex::new(VecDeque::new()));
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let result = {
            let callback_interrupt_requested = Arc::clone(&interrupt_requested);
            let callback_tool_call_active = Arc::clone(&tool_call_active);
            let run_steering_prompts = Arc::clone(&steering_prompts);
            let mut content = Vec::with_capacity(1 + images.len());
            if !prompt.is_empty() {
                content.push(ContentBlock::Text(prompt));
            }
            content.extend(images.into_iter().map(ContentBlock::Image));
            let mut run_future = Box::pin(agent.run_with_content_and_events_and_steering(
                content,
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
                        | AgentEvent::ToolUpdated { .. } => {}
                    }
                    let _ = event_tx.send(event);
                    if callback_interrupt_requested.load(Ordering::SeqCst) {
                        return Err(crate::model::ModelError::Interrupted);
                    }
                    Ok(())
                },
                move || Ok(run_steering_prompts.lock().unwrap().pop_front()),
            ));
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
                    Some(event) = event_rx.recv() => {
                        if let Err(err) = self.handle_queued_agent_event(event, terminal) {
                            break Err(crate::agent::AgentError::Provider(err));
                        }
                        match self.handle_running_terminal_events(
                            terminal,
                            &interrupt_requested,
                            &tool_call_active,
                        ) {
                            Ok(StreamControl::Interrupt) => {
                                break Err(crate::agent::AgentError::Provider(crate::model::ModelError::Interrupted));
                            }
                            Ok(StreamControl::Continue | StreamControl::Resize) => {}
                            Err(err) => break Err(crate::agent::AgentError::Provider(err)),
                        }
                        self.drain_steering_prompts_to(&steering_prompts);
                        self.resize_inline_viewport_if_needed(terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                    _ = tokio::time::sleep(LoadingSpinner::FRAME_INTERVAL) => {
                        match self.handle_running_terminal_events(
                            terminal,
                            &interrupt_requested,
                            &tool_call_active,
                        ) {
                            Ok(StreamControl::Interrupt) => {
                                break Err(crate::agent::AgentError::Provider(crate::model::ModelError::Interrupted));
                            }
                            Ok(StreamControl::Continue | StreamControl::Resize) => {}
                            Err(err) => break Err(crate::agent::AgentError::Provider(err)),
                        }
                        self.drain_steering_prompts_to(&steering_prompts);
                        self.resize_inline_viewport_if_needed(terminal)?;
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
        match result {
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
            }
            Err(crate::agent::AgentError::Provider(crate::model::ModelError::Interrupted)) => {
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams(terminal)?;
                self.insert_entry(terminal, &Entry::Notice("model interrupted".into()))?;
                self.reset_streams();
                self.current_turn_start = None;
                self.status = "interrupted".into();
            }
            Err(err) => {
                self.reset_streams();
                self.current_turn_start = None;
                self.running = false;
                self.loading_spinner.stop();
                self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                self.status = "error".into();
            }
        }
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
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
            self.status = "image paste is unavailable while a model turn is running".into();
            return;
        }
        if !matches!(self.composer, ComposerMode::Input) {
            self.status = "image paste is only available in the message box".into();
            return;
        }
        match read_clipboard_image() {
            Ok(image) => {
                let summary = image_summary(&image);
                self.pending_images.push(image);
                self.status = format!("attached image {} ({summary})", self.pending_images.len());
            }
            Err(err) => {
                self.status = format!("image paste failed: {err}");
            }
        }
    }

    fn insert_running_paste(&mut self, text: &str) {
        match &mut self.composer {
            ComposerMode::Input => self.insert_pasted_input_text(text),
            ComposerMode::SecretInput(secret) => secret.insert_text(text),
            ComposerMode::ConfigNumberInput(input) => input.insert_text(text),
            ComposerMode::ConfigTextInput(input) => input.insert_text(text),
            ComposerMode::Picker(_) | ComposerMode::OAuthPending(_) => {}
        }
    }

    fn handle_key_during_turn(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.handle_running_config_number_key(key, terminal)? {
            return Ok(());
        }
        if self.handle_running_config_text_key(key, terminal)? {
            return Ok(());
        }
        if self.handle_running_picker_key(key, terminal)? {
            return Ok(());
        }
        if self.handle_running_command_palette_key(key, terminal)? {
            return Ok(());
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    self.input.clear();
                    self.paste_segments.clear();
                    self.pending_images.clear();
                    self.input_cursor = 0;
                    self.clamp_command_selection();
                    self.status = "input cleared; press esc to interrupt model".into();
                    self.ctrl_c_streak = 1;
                } else {
                    self.should_quit = true;
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('v'))
            | (KeyModifiers::ALT, KeyCode::Char('v')) => {
                self.paste_clipboard_image();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('o')) => {
                self.toggle_latest_tool_output(terminal)?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                self.status = "reset is unavailable while a model turn is running".into();
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
                    self.status = format!(
                        "editing queued message; {} queued message(s) remain",
                        self.queued_prompts.len()
                    );
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
                self.queue_prompt_after_turn(terminal)?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_input_char('\n');
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Enter) => {
                if self
                    .paste_burst
                    .should_insert_newline_for_enter(Instant::now())
                {
                    self.insert_input_char('\n');
                } else {
                    self.submit_during_turn(terminal)?;
                }
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_input_char(ch);
                self.paste_burst.record_plain_char(Instant::now());
                self.ctrl_c_streak = 0;
            }
            _ => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
        }
        self.clamp_command_selection();
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

        match commands::parse_command(&self.input) {
            Ok(Some(invocation)) => {
                self.input.clear();
                self.paste_segments.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                self.execute_command_during_turn(invocation, terminal)?;
            }
            Ok(None) => {
                self.queue_steering_prompt(prompt, terminal)?;
            }
            Err(commands::CommandParseError::Unknown(name)) => {
                self.input.clear();
                self.paste_segments.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!(
                        "unknown or unavailable command '/{name}' while a model turn is running"
                    )),
                )?;
                self.status = "command unavailable while running".into();
            }
        }
        Ok(())
    }

    fn queue_steering_prompt(
        &mut self,
        prompt: String,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.reset_input_history_navigation();
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
        self.steering_prompts.push_back(prompt);
        self.insert_entry(
            terminal,
            &Entry::Notice(format!(
                "queued steer {} for after the current output or tool call",
                self.steering_prompts.len()
            )),
        )?;
        self.status = format!("queued {} steer(s)", self.steering_prompts.len());
        Ok(())
    }

    fn queue_prompt_after_turn(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
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
        self.queue_prompt(prompt, display_prompt, paste_segments, terminal)
    }

    fn queue_prompt(
        &mut self,
        prompt: String,
        display_prompt: String,
        paste_segments: Vec<PasteSegment>,
        terminal: &mut DefaultTerminal,
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
        self.insert_entry(
            terminal,
            &Entry::Notice(format!(
                "queued message {} for after the current turn",
                self.queued_prompts.len()
            )),
        )?;
        self.status = format!("queued {} message(s)", self.queued_prompts.len());
        Ok(())
    }

    fn execute_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Exit => self.execute_exit_command(terminal),
            CommandId::Config => self.execute_config_command(terminal),
            CommandId::Skills => self.execute_skills_command(terminal),
            CommandId::TitleModel => self.execute_title_model_command(invocation, terminal),
            CommandId::New
            | CommandId::Model
            | CommandId::RefreshModelList
            | CommandId::Login
            | CommandId::Logout
            | CommandId::Resume => {
                self.insert_entry(
                    terminal,
                    &Entry::Notice(format!(
                        "/{} is unavailable while a model turn is running",
                        invocation.name
                    )),
                )?;
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
                    let (input, cursor) = self.complete_command_choice(&choice);
                    self.input = input;
                    self.input_cursor = cursor;
                    self.command_palette_dismissed = false;
                    self.clamp_command_selection();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(choice) = self.selected_command() {
                    let (input, cursor) = self.complete_command_choice(&choice);
                    self.input = input;
                    self.input_cursor = cursor;
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

    fn handle_running_picker_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
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
            (KeyModifiers::NONE, KeyCode::Char(' ')) if self.picker_space_confirms_selection() => {
                self.submit_picker_selection_during_turn(terminal)?;
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let ComposerMode::Picker(picker) = &mut self.composer {
                    picker.push_filter_char(ch);
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.submit_picker_selection_during_turn(terminal)?;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.handle_picker_escape(/*running*/ true)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn submit_picker_selection_during_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
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
            PickerAction::Config => self.submit_config_selection_during_turn(&value, terminal)?,
            PickerAction::SelectTitleModel => {
                self.refresh_available_auths();
                let (provider, _model, auth) = self.title_model_selection();
                match catalog::resolve_model_selection_for_auths(
                    &value,
                    &provider,
                    &auth,
                    &self.available_auths,
                ) {
                    Ok(selection) => self.select_title_model(selection, terminal)?,
                    Err(err) => {
                        self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                        self.status = "title model switch failed".into();
                    }
                }
            }
            PickerAction::SelectModel
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::ResumeSession => {
                self.insert_entry(
                    terminal,
                    &Entry::Notice(
                        "that picker action is unavailable while a model turn is running".into(),
                    ),
                )?;
                self.status = "picker action unavailable while running".into();
            }
        }
        Ok(())
    }

    fn submit_config_selection_during_turn(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match value {
            config_picker::MAX_OUTPUT_BYTES_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxOutputBytes,
                    config.max_output_bytes,
                ));
                self.status = "edit max output bytes".into();
            }
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxToolOutputLines,
                    config.max_tool_output_lines,
                ));
                self.status = "edit max tool output lines".into();
            }
            config_picker::REASONING_VALUE => {
                self.insert_entry(
                    terminal,
                    &Entry::Notice(
                        "reasoning changes are unavailable while a model turn is running".into(),
                    ),
                )?;
                self.status = "config action unavailable while running".into();
            }
            config_picker::SHOW_REASONING_OUTPUT_VALUE => {
                self.toggle_reasoning_output(terminal)?;
            }
            config_picker::CHECK_FOR_UPDATES_VALUE => {
                self.toggle_check_for_updates(terminal)?;
            }
            config_picker::WEB_SEARCH_VALUE => {
                self.composer =
                    ComposerMode::Picker(config_picker::web_search_config_picker(&self.info));
                self.status = "web search config".into();
            }
            config_picker::WEB_SEARCH_BACK_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::Picker(config_picker::config_picker(
                    &self.info,
                    config.max_output_bytes,
                    config.max_tool_output_lines,
                ));
                self.status = "config".into();
            }
            config_picker::WEB_SEARCH_PROVIDER_VALUE => self.cycle_web_search_provider(terminal)?,
            config_picker::WEB_SEARCH_OPENAI_KEY_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigTextInput(ConfigTextInput::new(
                    ConfigTextKey::OpenAiSearch,
                    config.web_search_openai_api_key,
                ));
                self.status = "edit OpenAI web search API key".into();
            }
            config_picker::WEB_SEARCH_EXA_KEY_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigTextInput(ConfigTextInput::new(
                    ConfigTextKey::Exa,
                    config.web_search_exa_api_key,
                ));
                self.status = "edit Exa API key".into();
            }
            config_picker::WEB_SEARCH_BRAVE_KEY_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigTextInput(ConfigTextInput::new(
                    ConfigTextKey::Brave,
                    config.web_search_brave_api_key,
                ));
                self.status = "edit Brave Search API key".into();
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

    fn handle_running_config_text_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigTextInput(_)) {
            return Ok(false);
        }
        self.handle_config_text_key(key, terminal)
    }

    fn reset_streams(&mut self) {
        self.assistant_stream.reset();
        self.assistant_stream_in_code_block = false;
        self.reasoning_stream.reset();
        self.current_stream_kind = None;
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

    fn handle_running_terminal_events(
        &mut self,
        terminal: &mut DefaultTerminal,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
    ) -> Result<StreamControl, crate::model::ModelError> {
        let mut control = StreamControl::Continue;
        while event::poll(Duration::from_millis(0))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Esc && !self.running_escape_has_overlay_target() {
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                    self.handle_key_during_turn(key, terminal).map_err(|err| {
                        crate::model::ModelError::InvalidResponse(err.to_string())
                    })?;
                    if self.should_quit {
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                }
                Event::Paste(text) => {
                    let text = normalize_paste(&text);
                    self.insert_running_paste(&text);
                    self.paste_burst.clear();
                }
                Event::Resize(_, _) => {
                    self.reflow_history(terminal)?;
                    self.drain_streams(terminal)?;
                    control = StreamControl::Resize;
                }
                _ => {}
            }
        }
        Ok(control)
    }

    fn running_escape_has_overlay_target(&self) -> bool {
        self.command_palette_visible() || !matches!(self.composer, ComposerMode::Input)
    }

    fn request_running_interrupt(
        &mut self,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
    ) -> StreamControl {
        interrupt_requested.store(true, Ordering::SeqCst);
        if tool_call_active.load(Ordering::SeqCst) {
            self.status = "interrupt requested; waiting for tool result".into();
            StreamControl::Continue
        } else {
            StreamControl::Interrupt
        }
    }

    fn handle_agent_event(
        &mut self,
        event: AgentEvent,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<bool> {
        match event {
            AgentEvent::OutputDelta(text) => {
                let switched = self.switch_stream_kind(terminal, StreamKind::Assistant)?;
                self.assistant_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Assistant)?;
                Ok(switched || drained)
            }
            AgentEvent::ReasoningDelta(text) => {
                if !self.active_turn_show_reasoning_output {
                    return Ok(false);
                }
                let switched = self.switch_stream_kind(terminal, StreamKind::Reasoning)?;
                self.reasoning_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Reasoning)?;
                Ok(switched || drained)
            }
            other => {
                if matches!(
                    other,
                    AgentEvent::StepStarted(_)
                        | AgentEvent::ToolStarted { .. }
                        | AgentEvent::ToolFinished { .. }
                ) {
                    self.finish_streams(terminal)?;
                }
                if let Some(entry) = self.record_agent_event(other) {
                    self.insert_entry(terminal, &entry)?;
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
        if let Some(fragment) = fragment {
            self.insert_stream_fragment(terminal, fragment, kind)?;
            Ok(true)
        } else {
            Ok(false)
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

    fn insert_stream_fragment(
        &mut self,
        terminal: &mut DefaultTerminal,
        fragment: StreamFragment,
        kind: StreamKind,
    ) -> std::io::Result<()> {
        let render_text = fragment.render_text();
        if !render_text.is_empty() {
            let width = terminal.size()?.width as usize;
            let style = kind.style();
            let mut lines = Vec::new();
            if fragment.include_leading_blank() {
                lines.push(Line::raw(""));
            }
            let mut text_lines = Vec::new();
            if matches!(kind, StreamKind::Assistant) {
                push_wrapped_markdown(
                    &mut text_lines,
                    render_text,
                    padded_content_width(width),
                    &mut self.assistant_stream_in_code_block,
                );
            } else {
                push_wrapped_text(
                    &mut text_lines,
                    render_text,
                    padded_content_width(width),
                    style,
                    LineFill::Natural,
                );
            }
            lines.extend(text_lines.into_iter().map(pad_display_line));
            insert_history_lines(terminal, lines)?;
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
            CommandId::Exit => self.execute_exit_command(terminal),
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
            CommandId::Logout => {
                self.execute_logout_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Resume => self.execute_resume_command(invocation, terminal, agent),
            CommandId::Config => self.execute_config_command(terminal),
            CommandId::Skills => self.execute_skills_command(terminal),
        }
    }

    fn execute_exit_command(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        self.insert_entry(terminal, &Entry::Notice("exiting rho".into()))?;
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
        self.last_inserted_was_tool = false;
        self.reflow_history(terminal)?;
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
            registry::providers()
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
            self.insert_entry(
                terminal,
                &Entry::Notice(
                    "no refreshable providers are configured. run /login for a provider with model list support."
                        .into(),
                ),
            )?;
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
                    self.insert_entry(
                        terminal,
                        &Entry::Notice(format!(
                            "refreshed {} model list: {} models",
                            refresh.provider,
                            refresh.models.len()
                        )),
                    )?;
                }
                Err(err) => {
                    self.insert_entry(
                        terminal,
                        &Entry::Error(format!("failed to refresh {provider} model list: {err}")),
                    )?;
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
            Ok(selection) => self.select_model(selection, terminal, agent),
            Err(err) => {
                self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
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
            self.insert_entry(
                terminal,
                &Entry::Notice(
                    "no cached API models. run /refresh-model-list after signing in.".into(),
                ),
            )?;
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
            Ok(selection) => self.select_title_model(selection, terminal),
            Err(err) => {
                self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
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
        let picker = model_picker::title_model_picker(&provider, &model, &self.available_auths);

        if picker.items.is_empty() {
            self.insert_entry(
                terminal,
                &Entry::Notice(
                    "no cached API models. run /refresh-model-list after signing in.".into(),
                ),
            )?;
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
                    Ok(selection) => self.select_model(selection, terminal, agent),
                    Err(err) => {
                        self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
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
                    Ok(selection) => self.select_title_model(selection, terminal),
                    Err(err) => {
                        self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                        self.status = "title model switch failed".into();
                        Ok(())
                    }
                }
            }
            PickerAction::LoginProvider => {
                self.start_login_for_provider(&value, terminal, agent).await
            }
            PickerAction::LogoutProvider => self.logout_provider(&value, terminal, agent).await,
            PickerAction::InsertSkillCommand => {
                self.input = format!("/skill:{value}");
                self.input_cursor = self.input_char_len();
                self.command_palette_dismissed = true;
                self.status = "skill command inserted".into();
                Ok(())
            }
            PickerAction::ResumeSession => self.submit_resume_selection(&value, terminal, agent),
            PickerAction::Config => self.submit_config_selection(&value, terminal, agent),
        }
    }

    fn submit_config_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        match value {
            config_picker::REASONING_VALUE => self.cycle_reasoning(terminal, agent),
            config_picker::SHOW_REASONING_OUTPUT_VALUE => self.toggle_reasoning_output(terminal),
            config_picker::CHECK_FOR_UPDATES_VALUE => self.toggle_check_for_updates(terminal),
            config_picker::MAX_OUTPUT_BYTES_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxOutputBytes,
                    config.max_output_bytes,
                ));
                self.status = "edit max output bytes".into();
                Ok(())
            }
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxToolOutputLines,
                    config.max_tool_output_lines,
                ));
                self.status = "edit max tool output lines".into();
                Ok(())
            }
            config_picker::WEB_SEARCH_VALUE => {
                self.composer =
                    ComposerMode::Picker(config_picker::web_search_config_picker(&self.info));
                self.status = "web search config".into();
                Ok(())
            }
            config_picker::WEB_SEARCH_BACK_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::Picker(config_picker::config_picker(
                    &self.info,
                    config.max_output_bytes,
                    config.max_tool_output_lines,
                ));
                self.status = "config".into();
                Ok(())
            }
            config_picker::WEB_SEARCH_PROVIDER_VALUE => self.cycle_web_search_provider(terminal),
            config_picker::WEB_SEARCH_OPENAI_KEY_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigTextInput(ConfigTextInput::new(
                    ConfigTextKey::OpenAiSearch,
                    config.web_search_openai_api_key,
                ));
                self.status = "edit OpenAI web search API key".into();
                Ok(())
            }
            config_picker::WEB_SEARCH_EXA_KEY_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigTextInput(ConfigTextInput::new(
                    ConfigTextKey::Exa,
                    config.web_search_exa_api_key,
                ));
                self.status = "edit Exa API key".into();
                Ok(())
            }
            config_picker::WEB_SEARCH_BRAVE_KEY_VALUE => {
                let config = Config::load(self.info.config_path.clone())?;
                self.composer = ComposerMode::ConfigTextInput(ConfigTextInput::new(
                    ConfigTextKey::Brave,
                    config.web_search_brave_api_key,
                ));
                self.status = "edit Brave Search API key".into();
                Ok(())
            }
            _ => Ok(()),
        }
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
        let config = Config::load(self.info.config_path.clone())?;
        let mut picker = config_picker::config_picker(
            &self.info,
            config.max_output_bytes,
            config.max_tool_output_lines,
        );
        Self::restore_picker_position(&mut picker, selected_value, filter);
        self.composer = ComposerMode::Picker(picker);
        self.status = "config".into();
        Ok(())
    }

    fn refresh_web_search_config_picker(&mut self, selected_value: &str) {
        let filter = match &self.composer {
            ComposerMode::Picker(picker) => picker.filter.clone(),
            _ => String::new(),
        };
        let mut picker = config_picker::web_search_config_picker(&self.info);
        Self::restore_picker_position(&mut picker, selected_value, filter);
        self.composer = ComposerMode::Picker(picker);
    }

    fn handle_picker_escape(&mut self, running: bool) -> anyhow::Result<()> {
        if self.web_search_config_picker_is_open() {
            self.open_main_config_picker_selected(config_picker::WEB_SEARCH_VALUE)
        } else {
            self.composer = ComposerMode::Input;
            self.status = if running { "running" } else { "ready" }.into();
            Ok(())
        }
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

    fn cycle_reasoning(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let reasoning = self.info.reasoning.next();
        let provider = match build_provider(&self.info.provider, &self.info.model, reasoning) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!("could not update reasoning to {reasoning}: {err}")),
                )?;
                self.status = "reasoning change failed".into();
                return Ok(());
            }
        };
        agent.replace_provider(provider);
        self.info.reasoning = reasoning;
        let save_result = Config::load(self.info.config_path.clone()).and_then(|mut config| {
            config.reasoning = reasoning;
            config.save(self.info.config_path.clone())
        });
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            let config = Config::load(self.info.config_path.clone()).unwrap_or_default();
            self.info.show_reasoning_output = config.show_reasoning_output;
            self.refresh_main_config_picker(config_picker::REASONING_VALUE)?;
        }
        match save_result {
            Ok(()) => {
                self.status = format!("reasoning: {reasoning}");
            }
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!(
                        "reasoning set to {reasoning} for this session, but saving config failed: {err}"
                    )),
                )?;
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    fn toggle_check_for_updates(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let save_result = Config::load(self.info.config_path.clone()).and_then(|mut config| {
            config.check_for_updates = !config.check_for_updates;
            config.save(self.info.config_path.clone())?;
            Ok(config.check_for_updates)
        });
        match save_result {
            Ok(check_for_updates) => {
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
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!("could not save update check setting: {err}")),
                )?;
                self.status = "config save failed".into();
            }
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            self.refresh_main_config_picker(config_picker::CHECK_FOR_UPDATES_VALUE)?;
        }
        Ok(())
    }

    fn toggle_reasoning_output(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let show_reasoning_output = !self.info.show_reasoning_output;
        let save_result = Config::load(self.info.config_path.clone()).and_then(|mut config| {
            config.show_reasoning_output = show_reasoning_output;
            config.save(self.info.config_path.clone())
        });
        match save_result {
            Ok(()) => {
                self.info.show_reasoning_output = show_reasoning_output;
                self.status = if show_reasoning_output {
                    "reasoning output: shown".into()
                } else {
                    "reasoning output: hidden".into()
                };
            }
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!("could not save reasoning output setting: {err}")),
                )?;
                self.status = "config save failed".into();
            }
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            let config = Config::load(self.info.config_path.clone()).unwrap_or_default();
            self.info.show_reasoning_output = config.show_reasoning_output;
            self.refresh_main_config_picker(config_picker::SHOW_REASONING_OUTPUT_VALUE)?;
        }
        Ok(())
    }

    fn cycle_web_search_provider(&mut self, _terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let mut config = Config::load(self.info.config_path.clone())?;
        config.web_search_provider = match config.web_search_provider.as_str() {
            "auto" => "openai",
            "openai" => "exa",
            "exa" => "brave",
            "brave" => "disabled",
            "disabled" => "auto",
            _ => "auto",
        }
        .into();
        let provider = config.web_search_provider.clone();
        config.save(self.info.config_path.clone())?;
        self.refresh_web_search_config_picker(config_picker::WEB_SEARCH_PROVIDER_VALUE);
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

    fn select_model(
        &mut self,
        selection: ModelSelection,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let provider = selection.provider;
        let model = selection.model;
        let auth = selection.auth;
        let provider_model = format!("{provider}/{model}");
        let new_provider = match build_provider(&provider, &model, self.info.reasoning) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!("could not switch to {provider_model}: {err}")),
                )?;
                self.status = "model switch failed".into();
                return Ok(());
            }
        };

        agent.replace_provider(new_provider);
        self.info.provider = provider.clone();
        self.info.model = model.clone();
        self.info.auth = auth.clone();
        self.info.auth_unavailable = None;
        self.using_unavailable_provider = false;
        self.start_model_metadata_fetch(agent);
        match Config::load(self.info.config_path.clone()).and_then(|mut config| {
            config.provider = provider.clone();
            config.model = model.clone();
            config.auth = auth.clone();
            config.save(self.info.config_path.clone())
        }) {
            Ok(()) => {
                self.insert_entry(
                    terminal,
                    &Entry::Notice(format!(
                        "model switched to {provider_model} and saved to config"
                    )),
                )?;
                self.status = format!("model: {provider_model}");
            }
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!(
                        "model switched to {provider_model} for this session, but saving config failed: {err}"
                    )),
                )?;
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    fn select_title_model(
        &mut self,
        selection: ModelSelection,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let provider = selection.provider;
        let model = selection.model;
        let auth = selection.auth;
        let provider_model = format!("{provider}/{model}");
        self.info.title_provider = Some(provider.clone());
        self.info.title_model = Some(model.clone());
        self.info.title_auth = Some(auth.clone());
        match Config::load(self.info.config_path.clone()).and_then(|mut config| {
            config.title_provider = Some(provider.clone());
            config.title_model = Some(model.clone());
            config.title_auth = Some(auth.clone());
            config.save(self.info.config_path.clone())
        }) {
            Ok(()) => {
                self.insert_entry(
                    terminal,
                    &Entry::Notice(format!(
                        "session title model switched to {provider_model} and saved to config"
                    )),
                )?;
                self.status = format!("title model: {provider_model}");
            }
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!(
                        "session title model switched to {provider_model} for this session, but saving config failed: {err}"
                    )),
                )?;
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    fn refresh_available_auths(&mut self) {
        self.available_auths = available_auth_modes(self.credential_store.as_ref());
    }

    fn save_current_config(&self) -> anyhow::Result<()> {
        Config::load(self.info.config_path.clone()).and_then(|mut config| {
            config.provider = self.info.provider.clone();
            config.model = self.info.model.clone();
            config.auth = self.info.auth.clone();
            config.save(self.info.config_path.clone())
        })
    }

    fn execute_resume_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let session_id = invocation.args.trim();
        if !session_id.is_empty() {
            return self.submit_resume_selection(session_id, terminal, agent);
        }

        self.open_resume_picker(terminal)
    }

    fn open_resume_picker(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        match Session::list(&self.info.cwd) {
            Ok(sessions) if sessions.is_empty() => {
                self.insert_entry(
                    terminal,
                    &Entry::Notice("no saved sessions for this workspace".into()),
                )?;
                self.status = "no sessions".into();
            }
            Ok(sessions) => {
                let picker =
                    session_picker::session_picker(sessions, self.info.session_id.as_deref());
                if picker.items.is_empty() {
                    self.insert_entry(
                        terminal,
                        &Entry::Notice("no other saved sessions for this workspace".into()),
                    )?;
                    self.status = "no sessions".into();
                    return Ok(());
                }
                self.composer = ComposerMode::Picker(picker);
                self.status = "select session".into();
            }
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!("could not list sessions: {err}")),
                )?;
                self.status = "resume failed".into();
            }
        }
        Ok(())
    }

    fn submit_resume_selection(
        &mut self,
        session_id: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        match self.resume_session_by_id(session_id, terminal, agent) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.composer = ComposerMode::Input;
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!("could not resume session: {err}")),
                )?;
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
        let (session, history) = Session::open_by_id(&self.info.cwd, session_id)?;
        let full_id = session.id().to_string();
        let short_id = short_session_id(&full_id);

        agent.replace_history(history);
        agent.set_session_id(Some(full_id.clone()));
        agent.set_history_sink(SessionHistorySink::new(session));
        self.info.session_id = Some(full_id);
        self.composer = ComposerMode::Input;
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.reset_streams();
        self.running = false;
        self.cumulative_usage = None;
        self.latest_usage = None;
        self.current_context = None;
        let entries = transcript_entries_from_messages(agent.messages());
        let width = terminal.size()?.width as usize;
        let (_omitted, visible_entries) = recovered_history_tail(
            &entries,
            width,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.max_tool_output_lines,
        );
        self.transcript = visible_entries;
        self.last_inserted_was_tool = self.transcript.last().is_some_and(is_tool_entry);
        self.reflow_history(terminal)?;
        self.insert_entry(
            terminal,
            &Entry::Notice(format!("resumed session {short_id}")),
        )?;
        self.status = format!("resumed {short_id}");
        Ok(())
    }

    fn execute_config_command(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let config = Config::load(self.info.config_path.clone())?;
        self.info.max_tool_output_lines = config.max_tool_output_lines.max(1);
        self.info.show_reasoning_output = config.show_reasoning_output;
        self.composer = ComposerMode::Picker(config_picker::config_picker(
            &self.info,
            config.max_output_bytes,
            self.info.max_tool_output_lines,
        ));
        self.status = "config".into();
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }

    fn execute_skills_command(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let picker = skill_picker::skill_picker(crate::skills::discover(&self.info.cwd));
        if picker.items.is_empty() {
            self.insert_entry(terminal, &Entry::Notice("no skills loaded".into()))?;
            self.status = "skills".into();
            return Ok(());
        }

        self.composer = ComposerMode::Picker(picker);
        self.status = "select skill".into();
        Ok(())
    }

    fn execute_skill_command(
        &mut self,
        name: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<bool> {
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
        self.insert_entry(
            terminal,
            &Entry::Notice(format!(
                "loaded skill {} from {}",
                skill.name,
                skill.path.display()
            )),
        )?;
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
        }
        self.status = if expand {
            "tool output expanded".into()
        } else {
            "tool output collapsed".into()
        };
        self.reflow_history(terminal)
    }

    fn record_agent_event(&mut self, event: AgentEvent) -> Option<Entry> {
        match event {
            AgentEvent::StepStarted(step) => {
                self.reset_streams();
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
        }
    }

    fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = area.width as usize;
        let lines = self.active_lines_at_for_height(width, area.height as usize, Instant::now());
        let composer_line_count = self.composer_lines(width).len() as u16;
        let statusline_count = self.statusline_lines(width).len() as u16;
        let command_line_count = self.command_suggestion_lines(width).len() as u16;
        let lines_below_composer = composer_line_count
            .saturating_add(1)
            .saturating_add(statusline_count)
            .saturating_add(command_line_count);
        let composer_y = (lines.len() as u16)
            .saturating_sub(lines_below_composer)
            .min(area.height.saturating_sub(lines_below_composer));
        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default())
                .wrap(Wrap { trim: false }),
            area,
        );

        let cursor = self.composer_cursor_position(width);
        frame.set_cursor_position(Position {
            x: area.x.saturating_add(cursor.x),
            y: area.y.saturating_add(composer_y).saturating_add(cursor.y),
        });
    }

    fn active_lines(&self, width: usize) -> Vec<Line<'static>> {
        self.active_lines_at_for_height(width, INLINE_VIEWPORT_HEIGHT as usize, Instant::now())
    }

    #[cfg(test)]
    fn active_lines_for_height(&self, width: usize, viewport_height: usize) -> Vec<Line<'static>> {
        self.active_lines_at_for_height(width, viewport_height, Instant::now())
    }

    fn active_lines_at_for_height(
        &self,
        width: usize,
        viewport_height: usize,
        now: Instant,
    ) -> Vec<Line<'static>> {
        let divider_style = if matches!(self.composer, ComposerMode::Picker(_)) {
            Theme::input_prompt()
        } else {
            Theme::dim()
        };
        let divider = Line::styled("─".repeat(width.max(1)), divider_style);
        let composer_lines = self.composer_lines(width);
        let statusline_lines = self.statusline_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        let composer_height =
            composer_lines.len() + statusline_lines.len() + command_lines.len() + 2;
        let available_content = viewport_height.saturating_sub(composer_height);

        let mut content = Vec::new();
        if let Some(pending) = &self.pending_tool_call {
            let spinner_lines = usize::from(self.loading_active());
            let pending_tool_output_lines = self
                .info
                .max_tool_output_lines
                .min(available_content.saturating_sub(spinner_lines + 3).max(1));
            content.extend(entry_lines(
                &Entry::Tool(pending.clone()),
                width,
                pending_tool_output_lines,
            ));
        }
        if self.loading_active() {
            content.push(self.loading_spinner.line(now));
        }

        let mut lines = Vec::new();
        let skip = content.len().saturating_sub(available_content);
        lines.extend(content.into_iter().skip(skip));
        lines.push(divider.clone());
        lines.extend(composer_lines);
        lines.push(divider);
        lines.extend(statusline_lines);
        lines.extend(command_lines);
        lines
    }

    fn composer_lines(&self, width: usize) -> Vec<Line<'static>> {
        match &self.composer {
            ComposerMode::Input => {
                input_lines_with_images(&self.input, &self.pending_images, width)
            }
            ComposerMode::Picker(picker) => picker_lines(picker, width),
            ComposerMode::SecretInput(secret) => secret_input_lines(secret, width),
            ComposerMode::ConfigNumberInput(input) => config_number_input_lines(input, width),
            ComposerMode::ConfigTextInput(input) => config_text_input_lines(input, width),
            ComposerMode::OAuthPending(target) => oauth_pending_lines(target, width),
        }
    }

    fn desired_inline_viewport_height(&self, width: usize, terminal_height: u16) -> u16 {
        let height = self.active_lines(width).len() as u16;
        height.max(1).min(terminal_height.max(1))
    }

    fn resize_inline_viewport_if_needed(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        let size = terminal.size()?;
        let desired_height = self.desired_inline_viewport_height(size.width as usize, size.height);
        if desired_height == self.inline_viewport_height {
            return Ok(());
        }

        clear_terminal_for_history_reflow(terminal)?;
        *terminal = Terminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(desired_height),
            },
        )?;
        self.inline_viewport_height = desired_height;
        self.replay_history(terminal)
    }

    fn statusline_lines(&self, width: usize) -> Vec<Line<'static>> {
        statusline_lines(
            &StatusLineState::from_tui(
                &self.info,
                &self.status,
                self.cumulative_usage.clone(),
                self.latest_usage.clone(),
                self.current_context.clone(),
                self.model_metadata.clone(),
                self.pending_model_metadata.is_some(),
            ),
            width,
        )
    }

    fn composer_cursor_position(&self, width: usize) -> Position {
        match &self.composer {
            ComposerMode::Input => {
                let mut position = input_cursor_position(&self.input, self.input_cursor, width);
                position.y = position.y.saturating_add(self.pending_images.len() as u16);
                position
            }
            ComposerMode::SecretInput(secret) => Position {
                x: secret.cursor.min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigNumberInput(input) => Position {
                x: input.cursor.min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigTextInput(input) => Position {
                x: input.cursor.min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::OAuthPending(_) => Position { x: 0, y: 0 },
            ComposerMode::Picker(picker) => Position {
                x: picker
                    .filter
                    .chars()
                    .count()
                    .saturating_add(2)
                    .min(width.saturating_sub(1)) as u16,
                y: 0,
            },
        }
    }

    fn command_suggestion_lines(&self, width: usize) -> Vec<Line<'static>> {
        if !self.command_palette_visible() {
            return Vec::new();
        }

        let matches = self.command_matches();
        let selected_index = self.command_selection.min(matches.len().saturating_sub(1));
        let start = selected_index
            .saturating_add(1)
            .saturating_sub(MAX_COMMAND_SUGGESTIONS);

        matches
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
                let text = format!("{marker} {usage:<usage_width$} {description}");
                let style = if selected {
                    Theme::brand()
                } else {
                    Theme::dim()
                };
                styled_line(text, width.max(1), style, LineFill::Natural)
            })
            .collect()
    }

    fn insert_session_intro(&self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let width = terminal.size()?.width as usize;
        insert_history_lines(terminal, session_header_lines(&self.info, width))?;
        Ok(())
    }

    fn insert_recovered_history(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &Agent,
    ) -> std::io::Result<()> {
        let entries = transcript_entries_from_messages(agent.messages());
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
        let mut lines = Vec::new();
        if omitted > 0 {
            lines.extend(entry_lines(
                &Entry::Notice(format!(
                    "resumed session; showing last {} messages, omitted {omitted} earlier messages",
                    visible_entries.len()
                )),
                width,
                self.info.max_tool_output_lines,
            ));
        }
        lines.extend(transcript_lines(
            &visible_entries,
            width,
            self.info.max_tool_output_lines,
        ));

        insert_history_lines(terminal, lines)?;
        self.transcript = visible_entries;
        self.last_inserted_was_tool = self.transcript.last().is_some_and(is_tool_entry);
        Ok(())
    }

    fn exit_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = session_header_lines(&self.info, width);
        let mut previous_was_tool = false;
        for entry in &self.transcript {
            if previous_was_tool && is_tool_entry(entry) {
                lines.push(Line::raw(""));
            }
            lines.extend(entry_lines(entry, width, self.info.max_tool_output_lines));
            previous_was_tool = is_tool_entry(entry);
        }

        let divider = Line::styled("─".repeat(width.max(1)), Theme::dim());
        lines.push(divider.clone());
        if matches!(self.composer, ComposerMode::SecretInput(_)) {
            lines.push(Line::raw("[secret input omitted]"));
        } else if matches!(self.composer, ComposerMode::OAuthPending(_)) {
            lines.push(Line::raw("[oauth login pending]"));
        } else {
            lines.extend(input_lines_with_images(
                &self.input,
                &self.pending_images,
                width,
            ));
        }
        lines.push(divider);
        lines
    }

    fn insert_entry(
        &mut self,
        terminal: &mut DefaultTerminal,
        entry: &Entry,
    ) -> std::io::Result<()> {
        let width = terminal.size()?.width as usize;
        if self.last_inserted_was_tool && is_tool_entry(entry) {
            insert_history_lines(terminal, vec![Line::raw("")])?;
        }

        insert_history_lines(
            terminal,
            entry_lines(entry, width, self.info.max_tool_output_lines),
        )?;
        self.push_transcript_entry(entry.clone());
        self.last_inserted_was_tool = is_tool_entry(entry);
        Ok(())
    }

    fn reflow_history(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        clear_terminal_for_history_reflow(terminal)?;
        self.replay_history(terminal)
    }

    fn replay_history(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let width = terminal.size()?.width as usize;
        let mut lines = session_header_lines(&self.info, width);
        let mut previous_was_tool = false;
        for entry in &self.transcript {
            if previous_was_tool && is_tool_entry(entry) {
                lines.push(Line::raw(""));
            }
            lines.extend(entry_lines(entry, width, self.info.max_tool_output_lines));
            previous_was_tool = is_tool_entry(entry);
        }
        insert_history_lines(terminal, lines)?;
        self.last_inserted_was_tool = previous_was_tool;
        Ok(())
    }

    fn push_transcript_entry(&mut self, entry: Entry) {
        match entry {
            Entry::Assistant(text) => match self.transcript.last_mut() {
                Some(Entry::Assistant(previous)) => previous.push_str(&text),
                _ => self.transcript.push(Entry::Assistant(text)),
            },
            Entry::Reasoning(text) => match self.transcript.last_mut() {
                Some(Entry::Reasoning(previous)) => previous.push_str(&text),
                _ => self.transcript.push(Entry::Reasoning(text)),
            },
            other => self.transcript.push(other),
        }
    }
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

fn transcript_lines(
    entries: &[Entry],
    width: usize,
    max_tool_output_lines: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut previous_was_tool = false;
    for entry in entries {
        if previous_was_tool && is_tool_entry(entry) {
            lines.push(Line::raw(""));
        }
        lines.extend(entry_lines(entry, width, max_tool_output_lines));
        previous_was_tool = is_tool_entry(entry);
    }
    lines
}

fn transcript_entries_from_messages(messages: &[Message]) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut pending_tool_names = VecDeque::new();
    for message in messages {
        match message {
            Message::System(_) => {}
            Message::User(blocks) => {
                let text = render_message_blocks(blocks);
                if !text.is_empty() {
                    entries.push(Entry::User(text));
                }
            }
            Message::Assistant(blocks) => {
                let text = text_blocks(blocks);
                if !text.is_empty() {
                    entries.push(Entry::Assistant(text));
                }
                pending_tool_names.extend(blocks.iter().filter_map(|block| match block {
                    ContentBlock::ToolCall(call) => Some(call.name.clone()),
                    ContentBlock::Text(_) | ContentBlock::Image(_) => None,
                }));
            }
            Message::ToolResult(result) => {
                let name = pending_tool_names
                    .pop_front()
                    .unwrap_or_else(|| "tool".into());
                let display_style = ToolDisplayStyle::for_tool_name(&name);
                let mut display_lines = vec![name];
                if !result.content.trim().is_empty() {
                    display_lines.push(result.content.clone());
                }
                entries.push(Entry::Tool(ToolEntry {
                    state: ToolEntryState::Finished {
                        ok: result.ok,
                        display_style,
                    },
                    display_lines,
                    expanded: false,
                }));
            }
        }
    }
    entries
}

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
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        provider.send_turn(ModelRequest {
            messages: vec![
                Message::System(
                    "Generate a concise title for this chat session. Return only the title, no quotes, no punctuation at the end. Use 3 to 7 words."
                        .into(),
                ),
                Message::user_text(format!("First user message:\n\n{first_user_message}")),
            ],
            tools: Vec::new(),
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

fn config_number_input_lines(input: &ConfigNumberInput, width: usize) -> Vec<Line<'static>> {
    let label = input.key.label();
    vec![
        styled_line(
            truncate_one_line(&format!("edit {label}  enter save, esc cancel"), width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&input.value, width),
            width,
            Theme::text(),
            LineFill::Natural,
        ),
    ]
}

fn config_text_input_lines(input: &ConfigTextInput, width: usize) -> Vec<Line<'static>> {
    let masked = "•".repeat(input.value.chars().count());
    vec![
        styled_line(
            truncate_one_line(
                &format!("edit {}  enter save, esc cancel", input.key.label()),
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

fn print_exit_lines(lines: &[Line<'_>]) -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    stdout.write_all(b"\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[H")?;
    for line in lines {
        write_styled_line(&mut stdout, line)?;
        stdout.write_all(b"\n")?;
    }
    stdout.flush()
}

fn write_styled_line(stdout: &mut impl Write, line: &Line<'_>) -> std::io::Result<()> {
    for span in &line.spans {
        write_style(stdout, span.style)?;
        stdout.write_all(span.content.as_bytes())?;
        stdout.write_all(b"\x1b[0m")?;
    }
    Ok(())
}

fn write_style(stdout: &mut impl Write, style: Style) -> std::io::Result<()> {
    if style.add_modifier.contains(Modifier::BOLD) {
        stdout.write_all(b"\x1b[1m")?;
    }
    if style.add_modifier.contains(Modifier::DIM) {
        stdout.write_all(b"\x1b[2m")?;
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        stdout.write_all(b"\x1b[3m")?;
    }
    if let Some(color) = style.fg {
        write_color(stdout, color, /*foreground*/ true)?;
    }
    if let Some(color) = style.bg {
        write_color(stdout, color, /*foreground*/ false)?;
    }
    Ok(())
}

fn write_color(stdout: &mut impl Write, color: Color, foreground: bool) -> std::io::Result<()> {
    let code = match (foreground, color) {
        (_, Color::Reset) => {
            if foreground {
                39
            } else {
                49
            }
        }
        (true, Color::Black) => 30,
        (true, Color::Red) => 31,
        (true, Color::Green) => 32,
        (true, Color::Yellow) => 33,
        (true, Color::Blue) => 34,
        (true, Color::Magenta) => 35,
        (true, Color::Cyan) => 36,
        (true, Color::Gray) => 37,
        (true, Color::DarkGray) => 90,
        (true, Color::LightRed) => 91,
        (true, Color::LightGreen) => 92,
        (true, Color::LightYellow) => 93,
        (true, Color::LightBlue) => 94,
        (true, Color::LightMagenta) => 95,
        (true, Color::LightCyan) => 96,
        (true, Color::White) => 97,
        (false, Color::Black) => 40,
        (false, Color::Red) => 41,
        (false, Color::Green) => 42,
        (false, Color::Yellow) => 43,
        (false, Color::Blue) => 44,
        (false, Color::Magenta) => 45,
        (false, Color::Cyan) => 46,
        (false, Color::Gray) => 47,
        (false, Color::DarkGray) => 100,
        (false, Color::LightRed) => 101,
        (false, Color::LightGreen) => 102,
        (false, Color::LightYellow) => 103,
        (false, Color::LightBlue) => 104,
        (false, Color::LightMagenta) => 105,
        (false, Color::LightCyan) => 106,
        (false, Color::White) => 107,
        (true, Color::Indexed(index)) => {
            write!(stdout, "\x1b[38;5;{index}m")?;
            return Ok(());
        }
        (false, Color::Indexed(index)) => {
            write!(stdout, "\x1b[48;5;{index}m")?;
            return Ok(());
        }
        (true, Color::Rgb(red, green, blue)) => {
            write!(stdout, "\x1b[38;2;{red};{green};{blue}m")?;
            return Ok(());
        }
        (false, Color::Rgb(red, green, blue)) => {
            write!(stdout, "\x1b[48;2;{red};{green};{blue}m")?;
            return Ok(());
        }
    };
    write!(stdout, "\x1b[{code}m")
}

fn clear_terminal_for_history_reflow(terminal: &mut DefaultTerminal) -> std::io::Result<()> {
    // Codex handles terminal resize by rebuilding source-backed transcript
    // scrollback after clearing stale terminal-wrapped rows. Do the same here,
    // but avoid purging scrollback because rho runs inline after shell output
    // that it cannot reconstruct.
    let size = terminal.size()?;
    let mut stdout = std::io::stdout();
    stdout.write_all(b"\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[H")?;
    stdout.flush()?;

    // The ANSI clear homes the real cursor, but ratatui also tracks cursor and
    // inline viewport state internally. Update that state before resizing so
    // the replay starts at the top of the cleared terminal instead of at the
    // old inline viewport anchor.
    terminal.set_cursor_position(Position { x: 0, y: 0 })?;
    terminal.resize(Rect::new(0, 0, size.width, size.height))?;
    terminal.clear()
}

fn insert_history_lines(
    terminal: &mut DefaultTerminal,
    lines: Vec<Line<'static>>,
) -> std::io::Result<()> {
    // Ratatui's inline viewport tracks the real viewport anchor internally
    // (it is not necessarily at the bottom of the screen). Use its insertion
    // API so finalized chat is moved into terminal scrollback above the live
    // composer without guessing the viewport position.
    let height = lines.len().max(1) as u16;
    terminal.insert_before(height, |buf| {
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(buf.area, buf);
    })?;
    Ok(())
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
    // xterm modifyOtherKeys mode 2 helps terminals/tmux preserve modified Enter
    // without forcing printable shifted characters into base-key escape codes.
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

#[derive(Clone, Copy, Debug)]
enum HistoryDirection {
    Previous,
    Next,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{save_anthropic_api_key, save_openai_api_key, MemoryCredentialStore};
    use ratatui::{backend::TestBackend, Terminal, TerminalOptions, Viewport};

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

    fn test_app() -> App {
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
                session_id: None,
                open_resume_picker: false,
                config_path: None,
                auth_unavailable: None,
                update_notice: None,
                max_tool_output_lines: 10,
            },
            store,
        )
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

        assert!(line_text(&lines[0]).contains("working"), "{rendered}");
        assert!(!rendered.contains("hello"), "{rendered}");
        assert!(!rendered.contains("thinking"), "{rendered}");
    }

    #[test]
    fn active_lines_for_height_uses_actual_viewport_height() {
        let mut app = test_app();
        app.running = true;

        let small_lines = app.active_lines_for_height(40, 4);
        let default_lines = app.active_lines_for_height(40, INLINE_VIEWPORT_HEIGHT as usize);

        assert_eq!(line_text(&small_lines[0]), "─".repeat(40));
        assert!(line_text(&default_lines[0]).contains("working"));
    }

    #[test]
    fn loading_spinner_advances_frames() {
        let started_at = Instant::now();
        let spinner = LoadingSpinner {
            started_at: Some(started_at),
        };

        assert_eq!(spinner.frame_at(started_at), "⠋");
        assert_eq!(
            spinner.frame_at(started_at + LoadingSpinner::FRAME_INTERVAL),
            "⠙"
        );
    }

    #[test]
    fn active_lines_hide_spinner_when_idle() {
        let app = test_app();
        let rendered = app
            .active_lines_at_for_height(40, INLINE_VIEWPORT_HEIGHT as usize, Instant::now())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("working"), "{rendered}");
    }

    #[test]
    fn desired_inline_viewport_height_shrinks_to_live_lines() {
        let app = test_app();

        assert!(app.desired_inline_viewport_height(60, 24) < INLINE_VIEWPORT_HEIGHT);
    }

    #[test]
    fn draw_anchors_last_live_line_to_viewport_bottom() {
        let app = test_app();
        let height = app.desired_inline_viewport_height(60, 24);
        let backend = TestBackend::new(60, height);
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )
        .unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let bottom = buffer_row_text(terminal.backend().buffer(), height.saturating_sub(1));
        assert!(bottom.contains("ready"), "{bottom:?}");
    }

    #[test]
    fn command_palette_anchors_last_suggestion_to_viewport_bottom() {
        let mut app = test_app();
        app.input = "/m".into();
        app.input_cursor = 2;
        app.clamp_command_selection();
        let height = app.desired_inline_viewport_height(60, 24);
        let backend = TestBackend::new(60, height);
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )
        .unwrap();

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
        let height = app.desired_inline_viewport_height(40, 24);
        let backend = TestBackend::new(40, height);
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )
        .unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let bottom = buffer_row_text(terminal.backend().buffer(), height.saturating_sub(1));
        assert!(bottom.contains("ready"), "{bottom:?}");
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
        let input_index = lines.iter().position(|line| line == "/m").unwrap();
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
                session_id: None,
                open_resume_picker: false,
                config_path: None,
                auth_unavailable: None,
                update_notice: None,
                max_tool_output_lines: 10,
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
    fn picker_filters_by_regex_and_autocompletes() {
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

        for ch in "codex.*mini".chars() {
            picker.push_filter_char(ch);
        }

        assert_eq!(picker.matching_indices(), vec![1]);
        assert_eq!(
            picker.selected_item().unwrap().value,
            "openai-codex/gpt-5.4-mini"
        );
        picker.complete_filter();
        assert_eq!(picker.filter, regex::escape("openai-codex/gpt-5.4-mini"));
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
    fn web_search_config_restore_keeps_api_key_row_selected() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut app = test_app();
        app.info.config_path = Some(config_dir.path().join("config.toml"));
        let mut picker = config_picker::web_search_config_picker(&app.info);

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
        app.info.config_path = Some(config_dir.path().join("config.toml"));
        app.composer = ComposerMode::Picker(config_picker::web_search_config_picker(&app.info));

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
        app.info.config_path = Some(config_dir.path().join("config.toml"));
        app.composer = ComposerMode::Picker(config_picker::config_picker(&app.info, 12000, 10));

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
    fn paste_burst_treats_enter_as_newline() {
        let start = Instant::now();
        let mut burst = PasteBurst::default();
        burst.record_plain_char(start);
        burst.record_plain_char(start + Duration::from_millis(1));
        assert!(burst.should_insert_newline_for_enter(start + Duration::from_millis(2)));
    }

    #[test]
    fn paste_burst_expires_before_enter_submit() {
        let start = Instant::now();
        let mut burst = PasteBurst::default();
        burst.record_plain_char(start);
        burst.record_plain_char(start + Duration::from_millis(1));
        assert!(!burst.should_insert_newline_for_enter(
            start + PASTE_ENTER_SUPPRESSION + Duration::from_millis(2)
        ));
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
