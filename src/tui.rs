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

use crate::{
    agent::{Agent, AgentEvent},
    model::OpenAiProvider,
};

const INLINE_VIEWPORT_HEIGHT: u16 = 18;
const PASTE_BURST_GAP: Duration = Duration::from_millis(12);
const PASTE_ENTER_SUPPRESSION: Duration = Duration::from_millis(120);
const PASTE_BURST_MIN_CHARS: usize = 2;

pub struct TuiInfo {
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: String,
    pub reasoning_summary: String,
}

pub async fn run(agent: &mut Agent<OpenAiProvider>, info: TuiInfo) -> anyhow::Result<()> {
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
    reasoning_buffer: String,
    running: bool,
    paste_burst: PasteBurst,
}

#[derive(Clone, Debug)]
enum Entry {
    User(String),
    Assistant(String),
    Tool { name: String, content: String },
    Notice(String),
    Error(String),
}

#[derive(Default)]
struct PasteBurst {
    last_plain_char_at: Option<Instant>,
    plain_char_count: usize,
    suppress_enter_until: Option<Instant>,
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
            reasoning_buffer: String::new(),
            running: false,
            paste_burst: PasteBurst::default(),
        }
    }

    async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<()> {
        self.insert_session_intro(terminal)?;
        while !self.should_quit {
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.handle_key(key, terminal, agent).await?;
                    }
                    Event::Paste(text) => {
                        self.insert_input_text(&normalize_paste(&text));
                        self.paste_burst.clear();
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
        Ok(())
    }

    async fn handle_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent<OpenAiProvider>,
    ) -> anyhow::Result<()> {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    self.input.clear();
                    self.input_cursor = 0;
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
                self.insert_entry(terminal, &Entry::Notice("conversation reset".into()))?;
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
        Ok(())
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
    }

    fn insert_input_text(&mut self, text: &str) {
        let byte_index = self.input_byte_index(self.input_cursor);
        self.input.insert_str(byte_index, text);
        self.input_cursor += text.chars().count();
    }

    fn backspace_input(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let start = self.input_byte_index(self.input_cursor - 1);
        let end = self.input_byte_index(self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor -= 1;
    }

    fn delete_input(&mut self) {
        if self.input_cursor >= self.input_char_len() {
            return;
        }
        let start = self.input_byte_index(self.input_cursor);
        let end = self.input_byte_index(self.input_cursor + 1);
        self.input.replace_range(start..end, "");
    }

    fn delete_word_before_cursor(&mut self) {
        let start_cursor = previous_word_boundary(&self.input, self.input_cursor);
        let start = self.input_byte_index(start_cursor);
        let end = self.input_byte_index(self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor = start_cursor;
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
            return Ok(());
        }

        self.input.clear();
        self.input_cursor = 0;
        self.insert_entry(terminal, &Entry::User(prompt.clone()))?;
        self.stream_buffer.clear();
        self.reasoning_buffer.clear();
        self.status = "running".into();
        self.running = true;
        terminal.draw(|frame| self.draw(frame))?;

        let result = agent
            .run_with_events(prompt, |event| {
                if let Some(entry) = self.record_agent_event(event) {
                    self.insert_entry(terminal, &entry)?;
                }
                let _ = terminal.draw(|frame| self.draw(frame));
                if poll_interrupt()? {
                    return Err(crate::model::ModelError::Interrupted);
                }
                Ok(())
            })
            .await;

        match result {
            Ok(answer) => {
                self.stream_buffer.clear();
                self.reasoning_buffer.clear();
                self.running = false;
                self.insert_entry(terminal, &Entry::Assistant(answer))?;
                self.status = "ready".into();
            }
            Err(crate::agent::AgentError::Provider(crate::model::ModelError::Interrupted)) => {
                let partial = visible_assistant_stream(&self.stream_buffer)
                    .trim()
                    .to_string();
                if !partial.is_empty() {
                    self.insert_entry(terminal, &Entry::Assistant(partial))?;
                }
                self.stream_buffer.clear();
                self.reasoning_buffer.clear();
                self.running = false;
                self.insert_entry(terminal, &Entry::Notice("model interrupted".into()))?;
                self.status = "interrupted".into();
            }
            Err(err) => {
                self.stream_buffer.clear();
                self.reasoning_buffer.clear();
                self.running = false;
                self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                self.status = "error".into();
            }
        }
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
            AgentEvent::ToolFinished { name, content } => Some(Entry::Tool { name, content }),
        }
    }

    fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = area.width as usize;
        let lines = self.active_lines(width);
        let input_line_count = input_visual_lines(&self.input, width).len() as u16;
        let input_y = (lines.len() as u16)
            .saturating_sub(input_line_count + 1)
            .min(area.height.saturating_sub(input_line_count + 1));
        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default())
                .wrap(Wrap { trim: false }),
            area,
        );

        let cursor = input_cursor_position(&self.input, self.input_cursor, width);
        frame.set_cursor_position(Position {
            x: area.x.saturating_add(cursor.x),
            y: area.y.saturating_add(input_y).saturating_add(cursor.y),
        });
    }

    fn active_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut content = Vec::new();
        if !self.reasoning_buffer.is_empty() {
            push_wrapped_entry(
                &mut content,
                "reasoning",
                Color::Magenta,
                &self.reasoning_buffer,
                width,
            );
            content.push(Line::raw(""));
        }
        if self.running || !self.stream_buffer.is_empty() {
            push_wrapped_entry(
                &mut content,
                "rho",
                Color::Cyan,
                visible_assistant_stream(&self.stream_buffer),
                width,
            );
            content.push(Line::raw(""));
        }

        let divider = Line::styled(
            "─".repeat(width.max(1)),
            Style::default().fg(Color::DarkGray),
        );
        let input_lines = input_visual_lines(&self.input, width);
        let mut lines = Vec::new();
        let composer_height = input_lines.len() + 2;
        let available_content = (INLINE_VIEWPORT_HEIGHT as usize).saturating_sub(composer_height);
        let skip = content.len().saturating_sub(available_content);
        lines.extend(content.into_iter().skip(skip));
        lines.push(divider.clone());
        lines.extend(input_lines.into_iter().map(Line::raw));
        lines.push(divider);
        lines
    }

    fn insert_session_intro(&self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let width = terminal.size()?.width as usize;
        let lines = session_header_lines(&self.info, width);
        let height = lines.len().max(1) as u16;
        terminal.insert_before(height, |buf| {
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .render(buf.area, buf);
        })?;
        Ok(())
    }

    fn insert_entry(&self, terminal: &mut DefaultTerminal, entry: &Entry) -> std::io::Result<()> {
        let width = terminal.size()?.width as usize;
        let lines = entry_lines(entry, width);
        let height = lines.len().max(1) as u16;
        terminal.insert_before(height, |buf| {
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .render(buf.area, buf);
        })?;
        Ok(())
    }
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
    let mut lines = Vec::new();
    match entry {
        Entry::User(text) => push_wrapped_entry(&mut lines, "you", Color::Green, text, width),
        Entry::Assistant(text) => push_wrapped_entry(&mut lines, "rho", Color::Cyan, text, width),
        Entry::Tool { name, content } => push_wrapped_entry(
            &mut lines,
            &format!("tool:{name}"),
            Color::Yellow,
            content,
            width,
        ),
        Entry::Notice(text) => {
            push_wrapped_entry(&mut lines, "notice", Color::DarkGray, text, width)
        }
        Entry::Error(text) => push_wrapped_entry(&mut lines, "error", Color::Red, text, width),
    }
    lines.push(Line::raw(""));
    lines
}

fn push_wrapped_entry(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    color: Color,
    text: &str,
    width: usize,
) {
    let prefix = format!("{label}> ");
    let first_width = width.saturating_sub(prefix.chars().count()).max(1);
    let continuation_width = first_width;

    let mut emitted = false;
    for raw_line in text.lines() {
        let mut chunks = wrap_line(raw_line, first_width);
        if chunks.is_empty() {
            chunks.push(String::new());
        }
        for (index, chunk) in chunks.into_iter().enumerate() {
            if !emitted && index == 0 {
                lines.push(Line::from(vec![
                    Span::styled(
                        prefix.clone(),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(chunk),
                ]));
            } else {
                for continuation_chunk in wrap_line(&chunk, continuation_width) {
                    lines.push(Line::from(vec![
                        Span::styled(
                            prefix.clone(),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(continuation_chunk),
                    ]));
                }
            }
        }
        emitted = true;
    }

    if !emitted {
        lines.push(Line::from(vec![
            Span::styled(
                prefix,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(""),
        ]));
    }
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
