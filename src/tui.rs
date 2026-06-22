use std::{
    io::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
};
use ratatui::{
    layout::Position,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
    DefaultTerminal, Frame, TerminalOptions, Viewport,
};
mod login;
mod render;

use render::{
    byte_index_after_visual_lines, entry_lines, input_cursor_position, input_visual_lines,
    picker_lines, picker_matching_indices, push_wrapped_text, session_header_lines, styled_line,
    truncate_one_line, visible_picker_match_start, LineFill,
};

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
        reasoning_config_value, ModelError, UnavailableProvider,
    },
    session::Session,
    tool::ToolDisplayStyle,
};

const INLINE_VIEWPORT_HEIGHT: u16 = 18;
const PASTE_BURST_GAP: Duration = Duration::from_millis(12);
const PASTE_ENTER_SUPPRESSION: Duration = Duration::from_millis(120);
const PASTE_BURST_MIN_CHARS: usize = 2;
const MAX_COMMAND_SUGGESTIONS: usize = 5;

pub struct TuiInfo {
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: String,
    pub reasoning_summary: String,
    pub auth: String,
    pub session_id: Option<String>,
    pub config_path: Option<PathBuf>,
    pub auth_unavailable: Option<String>,
}

pub struct TuiResult {
    pub resume_session_id: Option<String>,
}

pub async fn run(agent: &mut Agent, info: TuiInfo) -> anyhow::Result<TuiResult> {
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
    last_inserted_was_tool: bool,
    command_selection: usize,
    command_prefix: Option<String>,
    command_palette_dismissed: bool,
    composer: ComposerMode,
    credential_store: Arc<dyn CredentialStore>,
    available_auths: Vec<String>,
    using_unavailable_provider: bool,
    pending_oauth_login: Option<PendingOAuthLogin>,
}

#[derive(Clone, Debug)]
enum ComposerMode {
    Input,
    Picker(UiPicker),
    SecretInput(SecretInput),
    OAuthPending(LoginTarget),
}

#[derive(Clone, Debug)]
struct UiPicker {
    title: String,
    help: String,
    items: Vec<PickerItem>,
    selected: usize,
    filter: String,
    action: PickerAction,
}

#[derive(Clone, Debug)]
struct PickerItem {
    label: String,
    description: String,
    value: String,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerAction {
    SelectModel,
    LoginProvider,
    LogoutProvider,
    InsertSkillCommand,
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
    Tool {
        ok: bool,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
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

impl UiPicker {
    fn new(
        title: impl Into<String>,
        help: impl Into<String>,
        items: Vec<PickerItem>,
        action: PickerAction,
    ) -> Self {
        Self {
            title: title.into(),
            help: help.into(),
            items,
            selected: 0,
            filter: String::new(),
            action,
        }
    }

    fn select_previous(&mut self) {
        let matches = self.matching_indices();
        if matches.is_empty() {
            return;
        }
        let position = matches
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        self.selected = if position == 0 {
            *matches.last().unwrap()
        } else {
            matches[position - 1]
        };
    }

    fn select_next(&mut self) {
        let matches = self.matching_indices();
        if matches.is_empty() {
            return;
        }
        let position = matches
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        self.selected = matches[(position + 1) % matches.len()];
    }

    fn push_filter_char(&mut self, ch: char) {
        self.filter.push(ch);
        self.select_first_match();
    }

    fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.select_first_match();
    }

    fn complete_filter(&mut self) {
        if let Some(item) = self.selected_item() {
            self.filter = regex::escape(&item.value);
        }
    }

    fn select_first_match(&mut self) {
        if let Some(index) = self.matching_indices().first().copied() {
            self.selected = index;
        }
    }

    fn matching_indices(&self) -> Vec<usize> {
        picker_matching_indices(&self.items, &self.filter)
    }

    fn selected_item(&self) -> Option<&PickerItem> {
        self.matching_indices()
            .contains(&self.selected)
            .then(|| self.items.get(self.selected))
            .flatten()
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
            last_inserted_was_tool: false,
            command_selection: 0,
            command_prefix: None,
            command_palette_dismissed: false,
            composer: ComposerMode::Input,
            credential_store,
            available_auths,
            using_unavailable_provider,
            pending_oauth_login: None,
        }
    }

    async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<TuiResult> {
        self.insert_session_intro(terminal)?;
        if self.info.auth_unavailable.is_some() {
            self.insert_entry(
                terminal,
                &Entry::Notice("no providers configured. run /login to sign in.".into()),
            )?;
        }
        while !self.should_quit {
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
                            ComposerMode::Picker(_) | ComposerMode::OAuthPending(_) => {}
                        }
                        self.paste_burst.clear();
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
        self.insert_composer_snapshot(terminal)?;
        Ok(TuiResult {
            resume_session_id: self.info.session_id,
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
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                agent.reset();
                self.info.session_id = None;
                agent.clear_message_sink();
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
            self.info.session_id = Some(session.id().to_string());
            agent.set_message_sink(move |message| session.append_message(message));
        }
        Ok(())
    }

    async fn submit(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let prompt = self.input.trim().to_string();
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
                self.input.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                if self.execute_skill_command(&name, terminal, agent)? {
                    return Ok(());
                }
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

        self.input.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
        self.ensure_session(agent)?;
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
                if poll_interrupt()? {
                    return Err(crate::model::ModelError::Interrupted);
                }
                Ok(())
            })
            .await;

        match result {
            Ok(answer) => {
                let remaining = self.unflushed_answer_text(&answer).to_string();
                self.stream_buffer.clear();
                self.reasoning_buffer.clear();
                self.running = false;
                self.insert_assistant_output(
                    terminal,
                    &remaining,
                    self.stream_flushed_text.is_empty(),
                )?;
                self.stream_flushed_text.clear();
                self.status = "ready".into();
            }
            Err(crate::agent::AgentError::Provider(crate::model::ModelError::Interrupted)) => {
                let partial = visible_assistant_stream(&self.stream_buffer).to_string();
                self.stream_buffer.clear();
                self.reasoning_buffer.clear();
                self.running = false;
                self.insert_assistant_output(
                    terminal,
                    partial.trim(),
                    self.stream_flushed_text.is_empty(),
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
        push_wrapped_text(
            &mut lines,
            &flushed,
            width,
            Style::default(),
            LineFill::Natural,
        );
        insert_history_lines(terminal, lines)?;
        Ok(())
    }

    fn unflushed_answer_text<'a>(&self, answer: &'a str) -> &'a str {
        answer
            .strip_prefix(&self.stream_flushed_text)
            .unwrap_or(answer)
    }

    fn insert_assistant_output(
        &mut self,
        terminal: &mut DefaultTerminal,
        text: &str,
        include_leading_blank: bool,
    ) -> std::io::Result<()> {
        let width = terminal.size()?.width as usize;
        let mut lines = Vec::new();
        if include_leading_blank {
            lines.push(Line::raw(""));
        }
        if !text.is_empty() {
            let mut text_lines = Vec::new();
            push_wrapped_text(
                &mut text_lines,
                text,
                padded_content_width(width),
                Style::default(),
                LineFill::Natural,
            );
            lines.extend(text_lines.into_iter().map(pad_display_line));
        }
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
            CommandId::Login => {
                self.execute_login_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Logout => {
                self.execute_logout_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Resume => self.execute_resume_command(invocation, terminal),
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
        let models = catalog::available_models_for_auths(&self.available_auths);
        let current = format!("{}/{}", self.info.provider, self.info.model);
        let mut items = models
            .into_iter()
            .map(|entry| {
                let value = format!("{}/{}", entry.provider, entry.model);
                let description =
                    if entry.provider == self.info.provider && entry.model == self.info.model {
                        "current".into()
                    } else if entry.display_name != entry.model {
                        entry.display_name
                    } else {
                        String::new()
                    };
                PickerItem {
                    description,
                    value: value.clone(),
                    label: value,
                }
            })
            .collect::<Vec<_>>();
        items.sort_by_key(|item| item.value != current);

        if items.is_empty() {
            self.insert_entry(
                terminal,
                &Entry::Notice("no providers configured. run /login to sign in.".into()),
            )?;
            self.status = "ready".into();
            return Ok(());
        }

        let mut picker = UiPicker::new(
            "select model",
            "type regex filter, tab complete, up/down select, enter confirm, esc cancel",
            items,
            PickerAction::SelectModel,
        );
        if let Some(index) = picker.items.iter().position(|item| item.value == current) {
            picker.selected = index;
        }
        self.composer = ComposerMode::Picker(picker);
        self.status = "select model".into();
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

        self.composer = ComposerMode::Input;
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
        }
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
        let new_provider = match build_provider(
            &provider,
            &model,
            reasoning_config_value(&self.info.reasoning_effort),
            reasoning_config_value(&self.info.reasoning_summary),
        ) {
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
    ) -> anyhow::Result<()> {
        let notice = if invocation.args.is_empty() {
            "interactive resume is not implemented yet. Exit and run rho --resume <session-id> to resume a saved session."
                .to_string()
        } else {
            format!(
                "interactive resume is not implemented yet. Exit and run rho --resume {} to resume that session.",
                invocation.args
            )
        };
        self.insert_entry(terminal, &Entry::Notice(notice))?;
        self.status = "resume help".into();
        Ok(())
    }

    fn execute_config_command(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let path = self
            .info
            .config_path
            .clone()
            .map(|path| path.display().to_string())
            .or_else(|| {
                Config::default_path()
                    .ok()
                    .map(|path| path.display().to_string())
            })
            .unwrap_or_else(|| "default config path unavailable".into());
        self.insert_entry(
            terminal,
            &Entry::Notice(format!(
                "config: {path}\nprovider: {}\nmodel: {}\nreasoning effort: {}\nreasoning summary: {}\nfull config UI is not implemented yet; edit the config file directly for now.",
                self.info.provider,
                self.info.model,
                self.info.reasoning_effort,
                self.info.reasoning_summary
            )),
        )?;
        self.status = "config".into();
        Ok(())
    }

    fn execute_skills_command(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let items = crate::skills::discover(&self.info.cwd)
            .into_iter()
            .map(|skill| PickerItem {
                label: skill.name.clone(),
                description: skill.description,
                value: skill.name,
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            self.insert_entry(terminal, &Entry::Notice("no skills loaded".into()))?;
            self.status = "skills".into();
            return Ok(());
        }

        self.composer = ComposerMode::Picker(UiPicker::new(
            "loaded skills",
            "enter inserts command, type regex filter, esc cancel",
            items,
            PickerAction::InsertSkillCommand,
        ));
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
            AgentEvent::ToolFinished {
                ok,
                display_style,
                display_lines,
                ..
            } => Some(Entry::Tool {
                ok,
                display_style,
                display_lines,
            }),
        }
    }

    fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = area.width as usize;
        let lines = self.active_lines(width);
        let composer_line_count = self.composer_lines(width).len() as u16;
        let command_line_count = self.command_suggestion_lines(width).len() as u16;
        let lines_below_composer = composer_line_count
            .saturating_add(1)
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
            push_wrapped_text(
                &mut content,
                &self.reasoning_buffer,
                width,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
                LineFill::Natural,
            );
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

        let divider = Line::styled(
            "─".repeat(width.max(1)),
            Style::default().fg(Color::DarkGray),
        );
        let composer_lines = self.composer_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        let mut lines = Vec::new();
        let composer_height = composer_lines.len() + command_lines.len() + 2;
        let available_content = (INLINE_VIEWPORT_HEIGHT as usize).saturating_sub(composer_height);
        let skip = content.len().saturating_sub(available_content);
        lines.extend(content.into_iter().skip(skip));
        lines.push(divider.clone());
        lines.extend(composer_lines);
        lines.push(divider);
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
            ComposerMode::OAuthPending(target) => oauth_pending_lines(target, width),
        }
    }

    fn composer_cursor_position(&self, width: usize) -> Position {
        match &self.composer {
            ComposerMode::Input => input_cursor_position(&self.input, self.input_cursor, width),
            ComposerMode::SecretInput(secret) => Position {
                x: secret.cursor.min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::OAuthPending(_) => Position { x: 0, y: 0 },
            ComposerMode::Picker(picker) => {
                let matching_indices = picker.matching_indices();
                let selected_position = matching_indices
                    .iter()
                    .position(|index| *index == picker.selected)
                    .unwrap_or(0);
                let start = visible_picker_match_start(picker, &matching_indices);
                Position {
                    x: 0,
                    y: selected_position
                        .saturating_sub(start)
                        .saturating_add(2)
                        .min(matching_indices.len().saturating_add(1))
                        as u16,
                }
            }
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

    fn insert_composer_snapshot(&self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let width = terminal.size()?.width as usize;
        let divider = Line::styled(
            "─".repeat(width.max(1)),
            Style::default().fg(Color::DarkGray),
        );
        let mut lines = Vec::new();
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
        insert_history_lines(terminal, lines)
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

        insert_history_lines(terminal, entry_lines(entry, width))?;
        self.last_inserted_was_tool = matches!(entry, Entry::Tool { .. });
        Ok(())
    }
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

fn complete_slash_command(input: &str, cursor: usize, name: &str) -> (String, usize) {
    let token_end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    let token_len = input[..token_end].chars().count();
    let args = input[token_end..].trim_start();
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

fn poll_interrupt() -> Result<bool, crate::model::ModelError> {
    if !event::poll(Duration::from_millis(0))? {
        return Ok(false);
    }
    let Event::Key(key) = event::read()? else {
        return Ok(false);
    };
    Ok(key.kind == KeyEventKind::Press && key.code == KeyCode::Esc)
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
                reasoning_effort: "low".into(),
                reasoning_summary: "auto".into(),
                auth: "api-key".into(),
                session_id: None,
                config_path: None,
                auth_unavailable: None,
            },
            store,
        )
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
            .flat_map(|entry| entry_lines(entry, 40))
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
    fn bash_tool_block_shows_command() {
        let lines = entry_lines(
            &test_tool_entry(true, &["bash", "cargo test", "ignored output"]),
            40,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("bash"));
        assert!(rendered.contains("cargo test"));
        assert!(!rendered.contains("tool:"));
    }

    #[test]
    fn read_file_tool_block_shows_file_name_only() {
        let lines = entry_lines(&test_tool_entry(true, &["read_file", "src/main.rs"]), 40);
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
            },
            40,
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
            },
            40,
        );

        assert_eq!(lines[1].spans[0].style.bg, Some(Color::Rgb(95, 36, 36)));
    }

    #[test]
    fn read_file_tool_block_shows_line_range_label() {
        let lines = entry_lines(
            &test_tool_entry(true, &["read_file", "src/file.rs:10-24"]),
            40,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("src/file.rs:10-24"));
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
                    description: "current".into(),
                    value: "model-a".into(),
                },
                PickerItem {
                    label: "model-b".into(),
                    description: String::new(),
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

        assert!(
            rendered.contains("select model  enter confirm"),
            "{rendered}"
        );
        assert!(rendered.contains("> model-a"), "{rendered}");
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
                reasoning_effort: "low".into(),
                reasoning_summary: "auto".into(),
                auth: "api-key".into(),
                session_id: None,
                config_path: None,
                auth_unavailable: None,
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
                    description: String::new(),
                    value: "openai/gpt-5.5".into(),
                },
                PickerItem {
                    label: "openai-codex/gpt-5.4-mini".into(),
                    description: String::new(),
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
    fn picker_lines_render_name_description_table_with_truncated_description() {
        let picker = UiPicker::new(
            "loaded skills",
            "enter inserts command",
            vec![PickerItem {
                label: "test-skill".into(),
                description: "this description is much too long for the available width".into(),
                value: "test-skill".into(),
            }],
            PickerAction::InsertSkillCommand,
        );

        let lines = picker_lines(&picker, 36);

        assert!(line_text(&lines[1]).contains("name"));
        assert!(line_text(&lines[1]).contains("| description"));
        assert!(line_text(&lines[2]).contains("test-skill"));
        assert!(line_text(&lines[2]).contains('…'));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn picker_selection_wraps() {
        let mut picker = UiPicker::new(
            "select model",
            "enter confirm",
            vec![
                PickerItem {
                    label: "model-a".into(),
                    description: String::new(),
                    value: "model-a".into(),
                },
                PickerItem {
                    label: "model-b".into(),
                    description: String::new(),
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
        let mut app = test_app();
        app.input = "/c".into();
        app.input_cursor = 2;
        app.clamp_command_selection();

        let lines = app.command_suggestion_lines(40);

        assert!(lines.iter().any(|line| line_text(line).contains('…')));
        assert!(lines
            .iter()
            .all(|line| line_text(line).chars().count() <= 40));
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
