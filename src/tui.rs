use std::{
    collections::VecDeque,
    future::Future,
    io::Write,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{task::noop_waker_ref, FutureExt};

use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
};
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
    DefaultTerminal, Frame, TerminalOptions, Viewport,
};
mod config_picker;
mod login;
mod model_picker;
mod picker;
mod provider_picker;
mod render;
mod session_picker;
mod skill_picker;
mod statusline;

use picker::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, UiPicker};
use render::{
    byte_index_after_visual_lines, entry_lines, input_cursor_position, input_visual_lines,
    picker_lines, push_wrapped_text, session_header_lines, styled_line, truncate_one_line,
    LineFill,
};
use statusline::{statusline_lines, StatusLineState};

use crate::{
    agent::{Agent, AgentEvent},
    auth::codex_oauth::{self, CodexOAuthError},
    commands::{self, CommandId, CommandInvocation, CommandSpec},
    config::Config,
    credentials::{
        available_auth_modes, delete_codex_tokens, delete_openai_api_key, provider_has_credentials,
        provider_has_env_override, save_codex_tokens, save_openai_api_key, CodexTokens,
        CredentialStore, OsCredentialStore,
    },
    model::{
        build_provider,
        catalog::{self, LoginTarget, ModelSelection},
        models_dev::{cached_model_metadata, fetch_model_metadata},
        ContentBlock, ContextUsage, Message, ModelError, ModelMetadata, ModelRequest,
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
const MAX_COMMAND_SUGGESTIONS: usize = 5;
const RECOVERED_HISTORY_LINE_LIMIT: usize = 200;

pub struct TuiInfo {
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
    pub reasoning: ReasoningLevel,
    pub auth: String,
    pub title_provider: Option<String>,
    pub title_model: Option<String>,
    pub title_auth: Option<String>,
    pub max_tool_output_lines: usize,
    pub session_id: Option<String>,
    pub open_resume_picker: bool,
    pub config_path: Option<PathBuf>,
    pub auth_unavailable: Option<String>,
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
    execute!(std::io::stdout(), EnableBracketedPaste)?;
    enable_modified_keys()?;
    execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    let result = App::new(info).run(&mut terminal, agent).await;
    execute!(std::io::stdout(), PopKeyboardEnhancementFlags)?;
    disable_modified_keys()?;
    execute!(std::io::stdout(), DisableBracketedPaste)?;
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
    stream_buffer: String,
    stream_flushed_text: String,
    reasoning_buffer: String,
    running: bool,
    paste_burst: PasteBurst,
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
    current_context: Option<ContextUsage>,
    model_metadata: Option<ModelMetadata>,
    pending_model_metadata: Option<tokio::task::JoinHandle<Option<ModelMetadata>>>,
    pending_session_title: Option<Pin<Box<dyn Future<Output = SessionTitleResult>>>>,
}

#[derive(Clone, Debug)]
enum ComposerMode {
    Input,
    Picker(UiPicker),
    SecretInput(SecretInput),
    ConfigNumberInput(ConfigNumberInput),
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

impl ConfigNumberKey {
    fn label(self) -> &'static str {
        match self {
            ConfigNumberKey::MaxOutputBytes => "max output bytes",
            ConfigNumberKey::MaxToolOutputLines => "max tool output lines",
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
    handle: tokio::task::JoinHandle<Result<CodexTokens, CodexOAuthError>>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum CommandChoiceKind {
    Builtin(&'static CommandSpec),
    Skill,
}

#[derive(Clone, Debug)]
enum Entry {
    User(String),
    #[allow(dead_code)]
    Assistant(String),
    Reasoning(String),
    Tool {
        ok: bool,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
        expanded: bool,
    },
    Notice(String),
    Error(String),
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
        Self {
            info,
            input: String::new(),
            input_cursor: 0,
            status,
            should_quit: false,
            ctrl_c_streak: 0,
            stream_buffer: String::new(),
            stream_flushed_text: String::new(),
            reasoning_buffer: String::new(),
            running: false,
            paste_burst: PasteBurst::default(),
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
            current_context: None,
            model_metadata: None,
            pending_model_metadata: None,
            pending_session_title: None,
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
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.handle_key(key, terminal, agent).await?;
                    }
                    Event::Paste(text) => {
                        let text = normalize_paste(&text);
                        match &mut self.composer {
                            ComposerMode::Input => self.insert_input_text(&text),
                            ComposerMode::SecretInput(secret) => secret.insert_text(&text),
                            ComposerMode::ConfigNumberInput(input) => input.insert_text(&text),
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
            (KeyModifiers::CONTROL, KeyCode::Char('o')) => {
                self.toggle_latest_tool_output(terminal)?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                agent.reset();
                self.info.session_id = None;
                agent.set_session_id(None);
                agent.clear_message_sink();
                agent.clear_history_replacement_sink();
                self.cumulative_usage = None;
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
            (_, KeyCode::Up) => {
                let width = terminal.size()?.width as usize;
                self.input_cursor = self.input_cursor.saturating_sub(width.max(1));
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Down) => {
                let width = terminal.size()?.width as usize;
                self.input_cursor = (self.input_cursor + width.max(1)).min(self.input_char_len());
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Home) => {
                self.input_cursor = 0;
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::End) => {
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
                if let Some(pending) = self.pending_oauth_login.take() {
                    pending.handle.abort();
                }
                self.composer = ComposerMode::Input;
                self.status = "login cancelled".into();
                self.insert_entry(terminal, &Entry::Notice("Codex login cancelled".into()))?;
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
                self.composer = ComposerMode::Input;
                self.status = "ready".into();
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

    fn insert_input_char(&mut self, ch: char) {
        let byte_index = self.input_byte_index(self.input_cursor);
        self.input.insert(byte_index, ch);
        self.input_cursor += 1;
        self.input_changed();
    }

    fn insert_input_text(&mut self, text: &str) {
        let byte_index = self.input_byte_index(self.input_cursor);
        self.input.insert_str(byte_index, text);
        self.input_cursor += text.chars().count();
        self.input_changed();
    }

    fn backspace_input(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let start = self.input_byte_index(self.input_cursor - 1);
        let end = self.input_byte_index(self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor -= 1;
        self.input_changed();
    }

    fn delete_input(&mut self) {
        if self.input_cursor >= self.input_char_len() {
            return;
        }
        let start = self.input_byte_index(self.input_cursor);
        let end = self.input_byte_index(self.input_cursor + 1);
        self.input.replace_range(start..end, "");
        self.input_changed();
    }

    fn delete_word_before_cursor(&mut self) {
        let start_cursor = previous_word_boundary(&self.input, self.input_cursor);
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
            let history_session = session.clone();
            agent.set_message_sink(move |message| session.append_message(message));
            agent.set_history_replacement_sink(move |messages| {
                history_session.replace_history(messages)
            });
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
        let mut prompt = self.input.trim().to_string();
        if prompt.is_empty() {
            self.input.clear();
            self.input_cursor = 0;
            self.clamp_command_selection();
            return Ok(());
        }

        match commands::parse_command(&self.input) {
            Ok(Some(invocation)) => {
                self.input.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                self.execute_command(invocation, terminal, agent).await?;
                return Ok(());
            }
            Ok(None) => {}
            Err(commands::CommandParseError::Unknown(name)) => {
                let trailing_prompt = slash_command_args(&self.input).trim().to_string();
                self.input.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                if self.execute_skill_command(&name, terminal, agent)? {
                    if trailing_prompt.is_empty() {
                        return Ok(());
                    }
                    prompt = trailing_prompt;
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

        self.input.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
        self.ensure_session(agent)?;
        if !agent
            .messages()
            .iter()
            .any(|message| matches!(message, Message::User(_)))
        {
            self.start_session_title_generation(prompt.clone());
        }
        self.insert_entry(terminal, &Entry::User(prompt.clone()))?;
        self.stream_buffer.clear();
        self.stream_flushed_text.clear();
        self.reasoning_buffer.clear();
        self.status = "running".into();
        self.running = true;
        terminal.draw(|frame| self.draw(frame))?;

        let result = agent
            .run_with_events(prompt, |event| {
                if let Some(entry) = self.record_agent_event(event) {
                    self.insert_entry(terminal, &entry)?;
                }
                self.flush_stream_overflow(terminal)?;
                let _ = terminal.draw(|frame| self.draw(frame));
                match poll_stream_control()? {
                    StreamControl::Interrupt => return Err(crate::model::ModelError::Interrupted),
                    StreamControl::Resize => self.reflow_history(terminal)?,
                    StreamControl::Continue => {}
                }
                Ok(())
            })
            .await;

        match result {
            Ok(answer) => {
                let remaining = self.unflushed_answer_text(&answer).to_string();
                let reasoning = std::mem::take(&mut self.reasoning_buffer);
                self.stream_buffer.clear();
                self.running = false;
                self.insert_reasoning_output(
                    terminal,
                    &reasoning,
                    self.stream_flushed_text.is_empty(),
                )?;
                self.insert_assistant_output(
                    terminal,
                    &remaining,
                    self.stream_flushed_text.is_empty() && reasoning.is_empty(),
                )?;
                self.stream_flushed_text.clear();
                self.status = "ready".into();
            }
            Err(crate::agent::AgentError::Provider(crate::model::ModelError::Interrupted)) => {
                let partial = visible_assistant_stream(&self.stream_buffer).to_string();
                let reasoning = std::mem::take(&mut self.reasoning_buffer);
                self.stream_buffer.clear();
                self.running = false;
                self.insert_reasoning_output(
                    terminal,
                    &reasoning,
                    self.stream_flushed_text.is_empty(),
                )?;
                self.insert_assistant_output(
                    terminal,
                    partial.trim(),
                    self.stream_flushed_text.is_empty() && reasoning.is_empty(),
                )?;
                self.stream_flushed_text.clear();
                self.insert_entry(terminal, &Entry::Notice("model interrupted".into()))?;
                self.status = "interrupted".into();
            }
            Err(err) => {
                self.stream_buffer.clear();
                self.stream_flushed_text.clear();
                self.reasoning_buffer.clear();
                self.running = false;
                self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                self.status = "error".into();
            }
        }
        Ok(())
    }

    fn flush_stream_overflow(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> Result<(), crate::model::ModelError> {
        let visible = visible_assistant_stream(&self.stream_buffer);
        if visible.is_empty() {
            return Ok(());
        }

        let width = terminal.size()?.width as usize;
        let composer_height =
            self.composer_lines(width).len() + self.command_suggestion_lines(width).len() + 2;
        let live_line_budget = (INLINE_VIEWPORT_HEIGHT as usize)
            .saturating_sub(composer_height)
            .saturating_sub(2)
            .max(1);
        let visual_lines = input_visual_lines(visible, width).len();
        let overflow = visual_lines.saturating_sub(live_line_budget);
        if overflow == 0 {
            return Ok(());
        }

        let Some(split_at) = byte_index_after_visual_lines(visible, width, overflow) else {
            return Ok(());
        };
        let flushed = self.stream_buffer[..split_at].to_string();
        self.stream_buffer.replace_range(..split_at, "");
        self.stream_flushed_text.push_str(&flushed);

        let mut lines = Vec::new();
        if self.stream_flushed_text == flushed {
            lines.push(Line::raw(""));
        }
        let mut flushed_lines = Vec::new();
        push_wrapped_text(
            &mut flushed_lines,
            &flushed,
            padded_content_width(width),
            Style::default(),
            LineFill::Natural,
        );
        lines.extend(flushed_lines.into_iter().map(pad_display_line));
        insert_history_lines(terminal, lines)?;
        self.push_transcript_entry(Entry::Assistant(flushed));
        Ok(())
    }

    fn unflushed_answer_text<'a>(&self, answer: &'a str) -> &'a str {
        answer
            .strip_prefix(&self.stream_flushed_text)
            .unwrap_or(answer)
    }

    fn insert_reasoning_output(
        &mut self,
        terminal: &mut DefaultTerminal,
        text: &str,
        include_leading_blank: bool,
    ) -> std::io::Result<()> {
        self.insert_padded_output(
            terminal,
            text,
            include_leading_blank,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )?;
        if !text.is_empty() {
            self.push_transcript_entry(Entry::Reasoning(text.into()));
        }
        Ok(())
    }

    fn insert_assistant_output(
        &mut self,
        terminal: &mut DefaultTerminal,
        text: &str,
        include_leading_blank: bool,
    ) -> std::io::Result<()> {
        self.insert_padded_output(terminal, text, include_leading_blank, Style::default())?;
        if !text.is_empty() {
            self.push_transcript_entry(Entry::Assistant(text.into()));
        }
        Ok(())
    }

    fn insert_padded_output(
        &mut self,
        terminal: &mut DefaultTerminal,
        text: &str,
        include_leading_blank: bool,
        style: Style,
    ) -> std::io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        let width = terminal.size()?.width as usize;
        let mut lines = Vec::new();
        if include_leading_blank {
            lines.push(Line::raw(""));
        }
        let mut text_lines = Vec::new();
        push_wrapped_text(
            &mut text_lines,
            text,
            padded_content_width(width),
            style,
            LineFill::Natural,
        );
        lines.extend(text_lines.into_iter().map(pad_display_line));
        lines.push(Line::raw(""));
        insert_history_lines(terminal, lines)
    }

    async fn execute_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Exit => self.execute_exit_command(terminal),
            CommandId::Model => {
                self.execute_model_command(invocation, terminal, agent)
                    .await
            }
            CommandId::TitleModel => self.execute_title_model_command(invocation, terminal),
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
                &Entry::Notice("no providers configured. run /login to sign in.".into()),
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
                &Entry::Notice("no providers configured. run /login to sign in.".into()),
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
            _ => Ok(()),
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
            self.composer = ComposerMode::Picker(config_picker::config_picker(
                &self.info,
                config.max_output_bytes,
                config.max_tool_output_lines,
            ));
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
        let history_session = session.clone();
        agent.set_message_sink(move |message| session.append_message(message));
        agent.set_history_replacement_sink(move |messages| {
            history_session.replace_history(messages)
        });
        self.info.session_id = Some(full_id);
        self.composer = ComposerMode::Input;
        self.input.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.stream_buffer.clear();
        self.stream_flushed_text.clear();
        self.reasoning_buffer.clear();
        self.running = false;
        self.cumulative_usage = None;
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
        self.last_inserted_was_tool = self
            .transcript
            .last()
            .is_some_and(|entry| matches!(entry, Entry::Tool { .. }));
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
        let Some(index) = self.transcript.iter().rposition(|entry| {
            matches!(entry, Entry::Tool { display_lines, .. } if tool_display_line_count(display_lines) > self.info.max_tool_output_lines)
        }) else {
            self.status = "no truncated tool output".into();
            return Ok(());
        };

        let expand = !matches!(
            self.transcript.get(index),
            Some(Entry::Tool { expanded: true, .. })
        );
        for entry in &mut self.transcript {
            if let Entry::Tool { expanded, .. } = entry {
                *expanded = false;
            }
        }
        if let Some(Entry::Tool { expanded, .. }) = self.transcript.get_mut(index) {
            *expanded = expand;
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
                self.stream_buffer.clear();
                self.stream_flushed_text.clear();
                self.reasoning_buffer.clear();
                self.running = true;
                self.status = format!("running step {step}");
                None
            }
            AgentEvent::OutputDelta(text) => {
                self.stream_buffer.push_str(&text);
                None
            }
            AgentEvent::ReasoningDelta(text) => {
                self.reasoning_buffer.push_str(&text);
                None
            }
            AgentEvent::ContextUsage(usage) => {
                self.current_context = Some(usage);
                None
            }
            AgentEvent::Usage(usage) => {
                let usage = usage_with_estimated_cost(usage, self.model_metadata.as_ref());
                merge_usage(&mut self.cumulative_usage, usage);
                None
            }
            AgentEvent::ToolFinished {
                ok,
                display_style,
                display_lines,
                ..
            } => Some(Entry::Tool {
                ok,
                display_style,
                display_lines,
                expanded: false,
            }),
        }
    }

    fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = area.width as usize;
        let lines = self.active_lines(width);
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
        let mut content = Vec::new();
        let visible_stream = visible_assistant_stream(&self.stream_buffer);
        let has_active_output =
            self.running || !self.reasoning_buffer.is_empty() || !visible_stream.is_empty();
        if has_active_output {
            content.push(Line::raw(""));
        }
        if !self.reasoning_buffer.is_empty() {
            let mut reasoning_lines = Vec::new();
            push_wrapped_text(
                &mut reasoning_lines,
                &self.reasoning_buffer,
                padded_content_width(width),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
                LineFill::Natural,
            );
            content.extend(reasoning_lines.into_iter().map(pad_display_line));
            content.push(Line::raw(""));
        }
        if !visible_stream.is_empty() {
            let mut stream_lines = Vec::new();
            push_wrapped_text(
                &mut stream_lines,
                visible_stream,
                padded_content_width(width),
                Style::default(),
                LineFill::Natural,
            );
            content.extend(stream_lines.into_iter().map(pad_display_line));
            content.push(Line::raw(""));
        }

        let divider_style = if matches!(self.composer, ComposerMode::Picker(_)) {
            Style::default().fg(Color::Blue)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let divider = Line::styled("─".repeat(width.max(1)), divider_style);
        let composer_lines = self.composer_lines(width);
        let statusline_lines = self.statusline_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        let mut lines = Vec::new();
        let composer_height =
            composer_lines.len() + statusline_lines.len() + command_lines.len() + 2;
        let available_content = (INLINE_VIEWPORT_HEIGHT as usize).saturating_sub(composer_height);
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
            ComposerMode::Input => input_visual_lines(&self.input, width)
                .into_iter()
                .map(Line::raw)
                .collect(),
            ComposerMode::Picker(picker) => picker_lines(picker, width),
            ComposerMode::SecretInput(secret) => secret_input_lines(secret, width),
            ComposerMode::ConfigNumberInput(input) => config_number_input_lines(input, width),
            ComposerMode::OAuthPending(target) => oauth_pending_lines(target, width),
        }
    }

    fn statusline_lines(&self, width: usize) -> Vec<Line<'static>> {
        statusline_lines(
            &StatusLineState::from_tui(
                &self.info,
                &self.status,
                self.cumulative_usage.clone(),
                self.current_context.clone(),
                self.model_metadata.clone(),
                self.pending_model_metadata.is_some(),
            ),
            width,
        )
    }

    fn composer_cursor_position(&self, width: usize) -> Position {
        match &self.composer {
            ComposerMode::Input => input_cursor_position(&self.input, self.input_cursor, width),
            ComposerMode::SecretInput(secret) => Position {
                x: secret.cursor.min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigNumberInput(input) => Position {
                x: input.cursor.min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::OAuthPending(_) => Position { x: 0, y: 0 },
            ComposerMode::Picker(picker) => Position {
                x: picker.filter.chars().count().saturating_add(2) as u16,
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
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
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
        self.last_inserted_was_tool = self
            .transcript
            .last()
            .is_some_and(|entry| matches!(entry, Entry::Tool { .. }));
        Ok(())
    }

    fn exit_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = session_header_lines(&self.info, width);
        let mut previous_was_tool = false;
        for entry in &self.transcript {
            if previous_was_tool && matches!(entry, Entry::Tool { .. }) {
                lines.push(Line::raw(""));
            }
            lines.extend(entry_lines(entry, width, self.info.max_tool_output_lines));
            previous_was_tool = matches!(entry, Entry::Tool { .. });
        }

        let divider = Line::styled(
            "─".repeat(width.max(1)),
            Style::default().fg(Color::DarkGray),
        );
        lines.push(divider.clone());
        if matches!(self.composer, ComposerMode::SecretInput(_)) {
            lines.push(Line::raw("[secret input omitted]"));
        } else if matches!(self.composer, ComposerMode::OAuthPending(_)) {
            lines.push(Line::raw("[oauth login pending]"));
        } else {
            lines.extend(
                input_visual_lines(&self.input, width)
                    .into_iter()
                    .map(Line::raw),
            );
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
        if self.last_inserted_was_tool && matches!(entry, Entry::Tool { .. }) {
            insert_history_lines(terminal, vec![Line::raw("")])?;
        }

        insert_history_lines(
            terminal,
            entry_lines(entry, width, self.info.max_tool_output_lines),
        )?;
        self.push_transcript_entry(entry.clone());
        self.last_inserted_was_tool = matches!(entry, Entry::Tool { .. });
        Ok(())
    }

    fn reflow_history(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        clear_terminal_for_history_reflow(terminal)?;
        let width = terminal.size()?.width as usize;
        let mut lines = session_header_lines(&self.info, width);
        let mut previous_was_tool = false;
        for entry in &self.transcript {
            if previous_was_tool && matches!(entry, Entry::Tool { .. }) {
                lines.push(Line::raw(""));
            }
            lines.extend(entry_lines(entry, width, self.info.max_tool_output_lines));
            previous_was_tool = matches!(entry, Entry::Tool { .. });
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
        let spacing = matches!(entry, Entry::Tool { .. }) && next_is_tool;
        let entry_line_count =
            entry_lines(entry, width, max_tool_output_lines).len() + usize::from(spacing);
        if selected_start < entries.len() && line_count + entry_line_count > line_limit {
            break;
        }
        selected_start = index;
        line_count += entry_line_count;
        next_is_tool = matches!(entry, Entry::Tool { .. });
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
        if previous_was_tool && matches!(entry, Entry::Tool { .. }) {
            lines.push(Line::raw(""));
        }
        lines.extend(entry_lines(entry, width, max_tool_output_lines));
        previous_was_tool = matches!(entry, Entry::Tool { .. });
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
                let text = text_blocks(blocks);
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
                    ContentBlock::Text(_) => None,
                }));
            }
            Message::ToolResult(result) => {
                let name = pending_tool_names
                    .pop_front()
                    .unwrap_or_else(|| "tool".into());
                let mut display_lines = vec![name];
                if !result.content.trim().is_empty() {
                    display_lines.push(result.content.clone());
                }
                entries.push(Entry::Tool {
                    ok: result.ok,
                    display_style: ToolDisplayStyle::default_tool(),
                    display_lines,
                    expanded: false,
                });
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
            ContentBlock::ToolCall(_) => None,
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
            format!(
                "enter API key for {}  enter save, esc cancel",
                secret.target.provider
            ),
            width,
            Style::default().fg(Color::DarkGray),
            LineFill::Natural,
        ),
        styled_line(masked, width, Style::default(), LineFill::Natural),
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
            format!("edit {label}  enter save, esc cancel"),
            width,
            Style::default().fg(Color::DarkGray),
            LineFill::Natural,
        ),
        styled_line(
            input.value.clone(),
            width,
            Style::default(),
            LineFill::Natural,
        ),
    ]
}

fn oauth_pending_lines(target: &LoginTarget, width: usize) -> Vec<Line<'static>> {
    vec![styled_line(
        format!("waiting for {} browser login  esc cancel", target.provider),
        width,
        Style::default().fg(Color::DarkGray),
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

fn visible_assistant_stream(text: &str) -> &str {
    let trimmed = text.trim_start();
    if (trimmed.starts_with("```json") && trimmed.contains("\"tool\""))
        || "Tool call: ".starts_with(trimmed)
        || trimmed.starts_with("Tool call: ")
    {
        ""
    } else {
        text
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamControl {
    Continue,
    Interrupt,
    Resize,
}

fn poll_stream_control() -> Result<StreamControl, crate::model::ModelError> {
    if !event::poll(Duration::from_millis(0))? {
        return Ok(StreamControl::Continue);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Esc => {
            Ok(StreamControl::Interrupt)
        }
        Event::Resize(_, _) => Ok(StreamControl::Resize),
        _ => Ok(StreamControl::Continue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryCredentialStore;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn test_tool_entry(ok: bool, display_lines: &[&str]) -> Entry {
        Entry::Tool {
            ok,
            display_style: ToolDisplayStyle::file_or_command(),
            display_lines: display_lines.iter().map(|line| (*line).into()).collect(),
            expanded: false,
        }
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
                auth: "api-key".into(),
                title_provider: None,
                title_model: None,
                title_auth: None,
                session_id: None,
                open_resume_picker: false,
                config_path: None,
                auth_unavailable: None,
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
    fn recovered_session_messages_become_transcript_entries() {
        let entries = transcript_entries_from_messages(&[
            Message::System("system".into()),
            Message::User(vec![ContentBlock::Text("hello".into())]),
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

        assert!(matches!(entries[0], Entry::User(ref text) if text == "hello"));
        assert!(matches!(entries[1], Entry::Assistant(ref text) if text == "hi"));
        assert!(matches!(
            entries[2],
            Entry::Tool { ok: false, ref display_lines, .. }
                if display_lines == &vec!["read_file".to_string(), "missing file".to_string()]
        ));
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
    fn skill_tool_block_shows_single_lavender_status_line() {
        let lines = entry_lines(
            &Entry::Tool {
                ok: true,
                display_style: ToolDisplayStyle::skill(),
                display_lines: vec!["skill caveman".into()],
                expanded: false,
            },
            40,
            10,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(lines[1].spans[0].style.bg, Some(Color::Rgb(92, 80, 140)));
        assert!(rendered.contains("skill caveman"));
        assert_eq!(rendered.matches("skill").count(), 1);
    }

    #[test]
    fn skill_tool_block_uses_red_failure_background() {
        let lines = entry_lines(
            &Entry::Tool {
                ok: false,
                display_style: ToolDisplayStyle::skill(),
                display_lines: vec!["unknown skill".into()],
                expanded: false,
            },
            40,
            10,
        );

        assert_eq!(lines[1].spans[0].style.bg, Some(Color::Rgb(95, 36, 36)));
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
        let Entry::Tool { expanded, .. } = &mut entry else {
            panic!("expected tool entry");
        };
        *expanded = true;

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
        if let Entry::Tool { expanded, .. } = &mut app.transcript[0] {
            *expanded = true;
        }

        let index = app
            .transcript
            .iter()
            .rposition(|entry| {
                matches!(entry, Entry::Tool { display_lines, .. } if tool_display_line_count(display_lines) > app.info.max_tool_output_lines)
            })
            .unwrap();
        for entry in &mut app.transcript {
            if let Entry::Tool { expanded, .. } = entry {
                *expanded = false;
            }
        }
        if let Entry::Tool { expanded, .. } = &mut app.transcript[index] {
            *expanded = true;
        }

        assert!(matches!(
            app.transcript[0],
            Entry::Tool {
                expanded: false,
                ..
            }
        ));
        assert!(matches!(
            app.transcript[1],
            Entry::Tool { expanded: true, .. }
        ));
    }

    #[test]
    fn step_started_clears_flushed_stream_state() {
        let mut app = test_app();
        app.stream_buffer = "current".into();
        app.stream_flushed_text = "previous".into();
        app.reasoning_buffer = "reasoning".into();

        assert!(app.record_agent_event(AgentEvent::StepStarted(2)).is_none());

        assert!(app.stream_buffer.is_empty());
        assert!(app.stream_flushed_text.is_empty());
        assert!(app.reasoning_buffer.is_empty());
        assert!(app.running);
        assert_eq!(app.status, "running step 2");
    }

    #[test]
    fn active_reasoning_has_side_padding() {
        let mut app = test_app();
        app.reasoning_buffer = "thinking".into();
        let lines = app.active_lines(40);

        assert!(lines.iter().any(|line| line_text(line) == " thinking "));
    }

    #[test]
    fn active_stream_keeps_stable_leading_spacer() {
        let mut app = test_app();
        app.running = true;
        let before_stream = app.active_lines(40);
        app.stream_buffer = "hello".into();
        let after_stream = app.active_lines(40);

        assert_eq!(line_text(&before_stream[0]), "");
        assert_eq!(line_text(&after_stream[0]), "");
        assert_eq!(line_text(&after_stream[1]), " hello ");
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
        let mut app = App::new_with_credentials(
            TuiInfo {
                cwd: PathBuf::from("/tmp/project"),
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                reasoning: ReasoningLevel::Low,
                auth: "api-key".into(),
                title_provider: None,
                title_model: None,
                title_auth: None,
                session_id: None,
                open_resume_picker: false,
                config_path: None,
                auth_unavailable: None,
                max_tool_output_lines: 10,
            },
            store,
        );
        app.refresh_available_auths();

        let models = catalog::available_models_for_auths(&app.available_auths);

        assert!(models.iter().any(|model| model.provider == "openai"));
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
    fn paste_normalization_converts_crlf_and_cr() {
        assert_eq!(normalize_paste("a\r\nb\rc"), "a\nb\nc");
    }
}
