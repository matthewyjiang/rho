use std::{
    io::Write,
    path::{Path, PathBuf},
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
use regex::RegexBuilder;

use crate::{
    agent::{Agent, AgentEvent},
    commands::{self, CommandId, CommandInvocation, CommandSpec},
    config::Config,
    model::{
        build_provider,
        catalog::{self, ModelSelection},
        reasoning_config_value, OpenAiProvider,
    },
    session::Session,
};

const INLINE_VIEWPORT_HEIGHT: u16 = 18;
const PASTE_BURST_GAP: Duration = Duration::from_millis(12);
const PASTE_ENTER_SUPPRESSION: Duration = Duration::from_millis(120);
const PASTE_BURST_MIN_CHARS: usize = 2;
const MAX_COMMAND_SUGGESTIONS: usize = 5;
const MAX_PICKER_ITEMS: usize = INLINE_VIEWPORT_HEIGHT as usize - 3;

pub struct TuiInfo {
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: String,
    pub reasoning_summary: String,
    pub auth: String,
    pub session_id: Option<String>,
    pub config_path: Option<PathBuf>,
}

pub struct TuiResult {
    pub resume_session_id: Option<String>,
}

pub async fn run(agent: &mut Agent<OpenAiProvider>, info: TuiInfo) -> anyhow::Result<TuiResult> {
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
}

#[derive(Clone, Debug)]
enum ComposerMode {
    Input,
    Picker(UiPicker),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerAction {
    SelectModel,
}

#[derive(Clone, Debug)]
enum Entry {
    User(String),
    #[allow(dead_code)]
    Assistant(String),
    Tool {
        name: String,
        command: Option<String>,
        ok: bool,
        content: String,
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
        Self {
            info,
            input: String::new(),
            input_cursor: 0,
            status: "ready".into(),
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
        }
    }

    async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<TuiResult> {
        self.insert_session_intro(terminal)?;
        while !self.should_quit {
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.handle_key(key, terminal, agent).await?;
                    }
                    Event::Paste(text) => {
                        if matches!(self.composer, ComposerMode::Input) {
                            self.insert_input_text(&normalize_paste(&text));
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
        agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<()> {
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

    async fn handle_picker_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent<OpenAiProvider>,
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
                self.submit_picker_selection(terminal, agent)?;
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
        agent: &mut Agent<OpenAiProvider>,
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
                if let Some(spec) = self.selected_command() {
                    let (input, cursor) =
                        commands::complete_command(&self.input, self.input_cursor, spec);
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
                if let Some(spec) = self.selected_command() {
                    let (input, cursor) =
                        commands::complete_command(&self.input, self.input_cursor, spec);
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

    fn command_matches(&self) -> Vec<&'static CommandSpec> {
        commands::command_prefix(&self.input)
            .map(commands::matching_commands)
            .unwrap_or_default()
    }

    fn selected_command(&self) -> Option<&'static CommandSpec> {
        let matches = self.command_matches();
        matches
            .get(self.command_selection.min(matches.len().saturating_sub(1)))
            .copied()
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

    fn ensure_session(&mut self, agent: &mut Agent<OpenAiProvider>) -> anyhow::Result<()> {
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
        agent: &mut Agent<OpenAiProvider>,
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
            Err(err) => {
                self.input.clear();
                self.input_cursor = 0;
                self.clamp_command_selection();
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!(
                        "{err}. Type / to choose one of: {}",
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
        push_wrapped_text(&mut lines, &flushed, width, Style::default(), false);
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
            push_wrapped_text(&mut lines, text, width, Style::default(), false);
        }
        lines.push(Line::raw(""));
        insert_history_lines(terminal, lines)
    }

    async fn execute_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Exit => self.execute_exit_command(terminal),
            CommandId::Model => {
                self.execute_model_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Login => self.execute_login_command(terminal),
            CommandId::Resume => self.execute_resume_command(invocation, terminal),
            CommandId::Config => self.execute_config_command(terminal),
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
        agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<()> {
        let model = invocation.args.trim();
        if model.is_empty() {
            self.open_model_picker(terminal, agent).await?;
            return Ok(());
        }

        match catalog::resolve_model_selection(model, &self.info.provider, &self.info.auth) {
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
        _agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<()> {
        self.status = "loading models".into();
        terminal.draw(|frame| self.draw(frame))?;
        let models = catalog::available_models(&self.info.auth);
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
                &Entry::Notice(
                    "model catalog has no available models for the current auth mode".into(),
                ),
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

    fn submit_picker_selection(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            self.composer = ComposerMode::Input;
            self.status = "ready".into();
            return Ok(());
        };

        self.composer = ComposerMode::Input;
        match action {
            PickerAction::SelectModel => {
                match catalog::resolve_model_selection(&value, &self.info.provider, &self.info.auth)
                {
                    Ok(selection) => self.select_model(selection, terminal, agent),
                    Err(err) => {
                        self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                        self.status = "model switch failed".into();
                        Ok(())
                    }
                }
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
        agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<()> {
        let provider = selection.provider;
        let model = selection.model;
        let auth = selection.auth;
        let provider_model = format!("{provider}/{model}");
        let new_provider = match build_provider(
            &provider,
            &model,
            &auth,
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

    fn execute_login_command(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        self.insert_entry(
            terminal,
            &Entry::Notice(
                "login UI is not implemented yet. For api-key auth, set OPENAI_API_KEY. For Codex auth, sign in with Codex and set auth = \"codex\" in the rho config."
                    .into(),
            ),
        )?;
        self.status = "login help".into();
        Ok(())
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

    fn record_agent_event(&mut self, event: AgentEvent) -> Option<Entry> {
        match event {
            AgentEvent::StepStarted(step) => {
                self.stream_buffer.clear();
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
                name,
                command,
                ok,
                content,
            } => Some(Entry::Tool {
                name,
                command,
                ok,
                content,
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
                false,
            );
            content.push(Line::raw(""));
        }
        if !visible_stream.is_empty() {
            push_wrapped_text(&mut content, visible_stream, width, Style::default(), false);
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
        }
    }

    fn composer_cursor_position(&self, width: usize) -> Position {
        match &self.composer {
            ComposerMode::Input => input_cursor_position(&self.input, self.input_cursor, width),
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
                        .saturating_add(1)
                        .min(matching_indices.len()) as u16,
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
                let text = format!("{marker} {:<16} {}", command.usage, command.description);
                let style = if selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                styled_line(text, width.max(1), style, false)
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
        lines.extend(
            input_visual_lines(&self.input, width)
                .into_iter()
                .map(Line::raw),
        );
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

fn session_header_lines(info: &TuiInfo, width: usize) -> Vec<Line<'static>> {
    let divider = "─".repeat(width.max(1));
    vec![
        Line::styled(divider.clone(), Style::default().fg(Color::DarkGray)),
        Line::from(vec![
            Span::styled(
                "rho",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  v"),
            Span::styled(
                env!("CARGO_PKG_VERSION"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("provider", Style::default().fg(Color::DarkGray)),
            Span::raw(": "),
            Span::styled(info.provider.clone(), Style::default().fg(Color::Yellow)),
            Span::raw("  •  model: "),
            Span::styled(info.model.clone(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("cwd", Style::default().fg(Color::DarkGray)),
            Span::raw(": "),
            Span::styled(compact_cwd(&info.cwd), Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::styled("reasoning", Style::default().fg(Color::DarkGray)),
            Span::raw(": "),
            Span::styled(
                format!("effort {}", info.reasoning_effort),
                Style::default().fg(Color::Magenta),
            ),
            Span::raw("  •  summary: "),
            Span::styled(
                info.reasoning_summary.clone(),
                Style::default().fg(Color::Magenta),
            ),
        ]),
        Line::styled(divider, Style::default().fg(Color::DarkGray)),
        Line::raw(""),
    ]
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

fn picker_lines(picker: &UiPicker, width: usize) -> Vec<Line<'static>> {
    let matching_indices = picker.matching_indices();
    let mut lines = Vec::with_capacity(matching_indices.len() + 2);
    let filter = if picker.filter.is_empty() {
        String::new()
    } else {
        format!("  filter: {}", picker.filter)
    };
    lines.push(styled_line(
        format!("{}  {}{}", picker.title, picker.help, filter),
        width,
        Style::default().fg(Color::DarkGray),
        false,
    ));
    if matching_indices.is_empty() {
        lines.push(styled_line(
            "  no matching models".to_string(),
            width,
            Style::default().fg(Color::DarkGray),
            false,
        ));
        return lines;
    }
    let start = visible_picker_match_start(picker, &matching_indices);
    for index in matching_indices
        .into_iter()
        .skip(start)
        .take(MAX_PICKER_ITEMS)
    {
        let item = &picker.items[index];
        let selected = index == picker.selected;
        let marker = if selected { ">" } else { " " };
        let text = if item.description.is_empty() {
            format!("{marker} {}", item.label)
        } else {
            format!("{marker} {:<28} {}", item.label, item.description)
        };
        let style = if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        lines.push(styled_line(text, width, style, false));
    }
    lines
}

fn visible_picker_match_start(picker: &UiPicker, matching_indices: &[usize]) -> usize {
    let selected_position = matching_indices
        .iter()
        .position(|index| *index == picker.selected)
        .unwrap_or(0);
    selected_position
        .saturating_add(1)
        .saturating_sub(MAX_PICKER_ITEMS)
}

fn picker_matching_indices(items: &[PickerItem], filter: &str) -> Vec<usize> {
    let filter = filter.trim();
    if filter.is_empty() {
        return (0..items.len()).collect();
    }

    let Ok(regex) = RegexBuilder::new(filter).case_insensitive(true).build() else {
        return Vec::new();
    };

    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let haystack = format!("{} {} {}", item.label, item.value, item.description);
            regex.is_match(&haystack).then_some(index)
        })
        .collect()
}

fn byte_index_after_visual_lines(text: &str, width: usize, target_lines: usize) -> Option<usize> {
    if target_lines == 0 {
        return Some(0);
    }

    let width = width.max(1);
    let mut completed = 0;
    let mut column = 0;
    for (index, ch) in text.char_indices() {
        let next = index + ch.len_utf8();
        if ch == '\n' {
            completed += 1;
            column = 0;
        } else {
            column += 1;
            if column >= width {
                completed += 1;
                column = 0;
            }
        }

        if completed >= target_lines {
            return Some(next);
        }
    }
    None
}

fn input_cursor_position(input: &str, cursor: usize, width: usize) -> Position {
    let prefix: String = input.chars().take(cursor).collect();
    let lines = input_visual_lines(&prefix, width);
    Position {
        x: lines
            .last()
            .map(|line| line.chars().count())
            .unwrap_or_default() as u16,
        y: lines.len().saturating_sub(1) as u16,
    }
}

fn input_visual_lines(input: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw_line in input.split('\n') {
        let wrapped = wrap_line(raw_line, width);
        if wrapped.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(wrapped);
        }
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
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

fn compact_cwd(path: &Path) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return path.display().to_string();
    };

    let home = Path::new(&home);
    if let Ok(rest) = path.strip_prefix(home) {
        let rel = rest.display().to_string();
        if rel.is_empty() {
            "~".to_string()
        } else {
            format!("~/{rel}")
        }
    } else {
        path.display().to_string()
    }
}

fn entry_lines(entry: &Entry, width: usize) -> Vec<Line<'static>> {
    let inner_width = padded_inner_width(width);
    let mut lines = Vec::new();
    match entry {
        Entry::User(text) => push_wrapped_text(
            &mut lines,
            text,
            inner_width,
            Style::default().fg(Color::White).bg(Color::Rgb(36, 44, 54)),
            true,
        ),
        Entry::Assistant(text) => {
            push_wrapped_text(&mut lines, text, inner_width, Style::default(), false)
        }
        Entry::Tool {
            name,
            command,
            ok,
            content,
        } => push_tool_block(
            &mut lines,
            name,
            command.as_deref(),
            *ok,
            content,
            inner_width,
        ),
        Entry::Notice(text) => push_wrapped_text(
            &mut lines,
            text,
            inner_width,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
            false,
        ),
        Entry::Error(text) => push_wrapped_text(
            &mut lines,
            text,
            inner_width,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            false,
        ),
    }

    let block_style = lines
        .first()
        .and_then(|line| line.spans.first())
        .map(|span| span.style)
        .unwrap_or_default();
    let mut padded = Vec::with_capacity(lines.len() + 2);
    padded.push(styled_blank_line(width, block_style));
    padded.extend(lines.into_iter().map(pad_line));
    padded.push(styled_blank_line(width, block_style));
    padded
}

fn push_tool_block(
    lines: &mut Vec<Line<'static>>,
    name: &str,
    command: Option<&str>,
    ok: bool,
    content: &str,
    width: usize,
) {
    let style = if matches!(name, "bash" | "read_file" | "write_file") {
        if ok {
            Style::default().fg(Color::White).bg(Color::Rgb(25, 75, 45))
        } else {
            Style::default().fg(Color::White).bg(Color::Rgb(95, 36, 36))
        }
    } else {
        Style::default()
            .fg(Color::Yellow)
            .bg(Color::Rgb(48, 45, 30))
    };

    push_wrapped_text(lines, name, width, style, true);
    if name == "bash" {
        if let Some(command) = command.filter(|command| !command.trim().is_empty()) {
            push_wrapped_text(lines, command, width, style, true);
        }
        if !content.trim().is_empty() {
            push_wrapped_text(lines, content, width, style, true);
        }
    } else if !content.trim().is_empty() {
        push_wrapped_text(lines, content, width, style, true);
    }
}

fn push_wrapped_text(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    fill_width: bool,
) {
    let width = width.max(1);
    let mut emitted = false;
    for raw_line in text.lines() {
        let chunks = wrap_line(raw_line, width);
        for chunk in chunks {
            lines.push(styled_line(chunk, width, style, fill_width));
            emitted = true;
        }
    }

    if !emitted {
        lines.push(styled_line(String::new(), width, style, fill_width));
    }
}

fn styled_line(mut text: String, width: usize, style: Style, fill_width: bool) -> Line<'static> {
    if fill_width {
        let len = text.chars().count();
        if len < width {
            text.push_str(&" ".repeat(width - len));
        }
    }
    Line::from(Span::styled(text, style))
}

fn padded_inner_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

fn pad_line(line: Line<'static>) -> Line<'static> {
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

fn styled_blank_line(width: usize, style: Style) -> Line<'static> {
    Line::from(Span::styled(" ".repeat(width.max(1)), style))
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        current.push(ch);
        if current.chars().count() >= width {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn test_app() -> App {
        App::new(TuiInfo {
            cwd: PathBuf::from("/tmp/project"),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            reasoning_effort: "low".into(),
            reasoning_summary: "auto".into(),
            auth: "api-key".into(),
            session_id: None,
            config_path: None,
        })
    }

    #[test]
    fn transcript_entries_render_without_prefix_labels() {
        let entries = [
            Entry::User("hello?".into()),
            Entry::Assistant("hi".into()),
            Entry::Tool {
                name: "read_file".into(),
                command: None,
                ok: true,
                content: "read src/main.rs".into(),
            },
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
            &Entry::Tool {
                name: "bash".into(),
                command: Some("cargo test".into()),
                ok: true,
                content: "ignored output".into(),
            },
            40,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("bash"));
        assert!(rendered.contains("cargo test"));
        assert!(!rendered.contains("tool:"));
    }

    #[test]
    fn read_file_tool_block_shows_file_name_only() {
        let lines = entry_lines(
            &Entry::Tool {
                name: "read_file".into(),
                command: None,
                ok: true,
                content: "src/main.rs".into(),
            },
            40,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("src/main.rs"));
    }

    #[test]
    fn read_file_tool_block_shows_line_range_label() {
        let lines = entry_lines(
            &Entry::Tool {
                name: "read_file".into(),
                command: None,
                ok: true,
                content: "src/file.rs:10-24".into(),
            },
            40,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("src/file.rs:10-24"));
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
        assert_eq!(line_text(&after_stream[1]), "hello");
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
        assert_eq!(app.command_selection, commands::COMMANDS.len() - 1);

        app.input = "/mo".into();
        app.input_cursor = 3;
        app.clamp_command_selection();
        assert_eq!(app.command_selection, 0);
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
