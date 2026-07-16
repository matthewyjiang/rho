use std::{
    fs::File,
    io::{IsTerminal, Read, Seek, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use rho_sdk::model::{ContextUsage, ModelUsage};
use serde::{Deserialize, Serialize};

use crate::{
    herdr::{HerdrReporter, HerdrState},
    subagent::{self, RunState, RunStatus},
    tool::ToolDisplayStyle,
};

use super::{
    event_adapter::{SdkEventAdapter, ViewEvent, ViewModelEvent},
    render::{entry_lines, truncate_one_line},
    terminal_events::TerminalEvents,
    theme::Theme,
    Entry, ToolEntry, ToolEntryState,
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_TOOL_OUTPUT_LINES: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum AttachmentEvent {
    Prompt(String),
    AssistantTextDelta(String),
    ReasoningDelta(String),
    ToolStarted {
        display_lines: Vec<String>,
    },
    ToolUpdated {
        display_lines: Vec<String>,
    },
    ToolFinished {
        ok: bool,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
    },
    Notice(String),
    ContextUsage(ContextUsage),
    Usage(ModelUsage),
    StepStarted,
    ProviderStreamReset,
    Completed,
    Cancelled,
    Failed(String),
}

/// Persists the same presentation events consumed by the interactive TUI so a
/// separate `rho attach` process can render a subagent without controlling it.
pub(crate) struct AttachmentWriter {
    file: File,
    adapter: SdkEventAdapter,
}

impl AttachmentWriter {
    pub(crate) fn new(result_path: &Path, cwd: PathBuf, prompt: &str) -> anyhow::Result<Self> {
        let path = result_path.with_file_name(subagent::ATTACHMENT_FILE_NAME);
        let file = subagent::create_private_file(&path)?;
        let mut writer = Self {
            file,
            adapter: SdkEventAdapter::new(cwd),
        };
        writer.write(&AttachmentEvent::Prompt(prompt.to_string()))?;
        Ok(writer)
    }

    pub(crate) fn on_event(&mut self, event: &rho_sdk::RunEvent) -> anyhow::Result<()> {
        let attachment = match self.adapter.translate(event.clone()) {
            ViewEvent::Update(update) => attachment_update(update),
            ViewEvent::Notice(notice) => Some(AttachmentEvent::Notice(notice)),
            ViewEvent::Questionnaire(request) => Some(AttachmentEvent::Notice(format!(
                "input requested but unavailable in a subagent: {}",
                request.title()
            ))),
            ViewEvent::Completed => Some(AttachmentEvent::Completed),
            ViewEvent::Cancelled => Some(AttachmentEvent::Cancelled),
            ViewEvent::Failed(message) => Some(AttachmentEvent::Failed(message)),
            ViewEvent::Ignored => None,
        };
        if let Some(attachment) = attachment {
            self.write(&attachment)?;
        }
        Ok(())
    }

    fn write(&mut self, event: &AttachmentEvent) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        Ok(())
    }
}

fn attachment_update(update: ViewModelEvent) -> Option<AttachmentEvent> {
    match update {
        ViewModelEvent::OutputDelta(text) => Some(AttachmentEvent::AssistantTextDelta(text)),
        ViewModelEvent::ReasoningDelta(text) => Some(AttachmentEvent::ReasoningDelta(text)),
        ViewModelEvent::ToolStarted { display_lines }
        | ViewModelEvent::ToolCallUpdated { display_lines } => {
            Some(AttachmentEvent::ToolStarted { display_lines })
        }
        ViewModelEvent::ToolUpdated { display_lines } => {
            Some(AttachmentEvent::ToolUpdated { display_lines })
        }
        ViewModelEvent::ToolFinished {
            ok,
            display_style,
            display_lines,
        } => Some(AttachmentEvent::ToolFinished {
            ok,
            display_style,
            display_lines,
        }),
        ViewModelEvent::RunStarted => None,
        ViewModelEvent::StepStarted(_) => Some(AttachmentEvent::StepStarted),
        ViewModelEvent::ProviderStreamReset => Some(AttachmentEvent::ProviderStreamReset),
        ViewModelEvent::ContextUsage(usage) => Some(AttachmentEvent::ContextUsage(usage)),
        ViewModelEvent::Usage(usage) => Some(AttachmentEvent::Usage(usage)),
    }
}

pub(crate) async fn run(id: &str, herdr: HerdrReporter) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("rho attach requires an interactive terminal");
    }
    let directory = subagent::directory(id)?;
    if !directory.is_dir() {
        anyhow::bail!("unknown subagent '{id}'");
    }
    subagent::secure_directory(&directory)?;

    let mut terminal = ratatui::init();
    let _restore_terminal = RestoreTerminal;
    Theme::initialize_from_terminal();
    let message = format!("attached to subagent {id}");
    herdr
        .report_state(HerdrState::Working, Some(&message), Some(id))
        .await;
    let result = AttachmentApp::new(id, directory, herdr.clone())
        .run(&mut terminal)
        .await;
    herdr.release().await;
    result
}

struct RestoreTerminal;

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

struct AttachmentReader {
    path: PathBuf,
    file: Option<File>,
    offset: u64,
    pending: Vec<u8>,
}

impl AttachmentReader {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            offset: 0,
            pending: Vec::new(),
        }
    }

    fn read_new(&mut self) -> anyhow::Result<Vec<AttachmentEvent>> {
        if self.file.is_none() {
            self.file = match File::open(&self.path) {
                Ok(file) => Some(file),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Vec::new());
                }
                Err(error) => return Err(error.into()),
            };
        }
        let file = self.file.as_mut().expect("attachment file opened");
        if file.metadata()?.len() < self.offset {
            file.seek(std::io::SeekFrom::Start(0))?;
            self.offset = 0;
            self.pending.clear();
        }
        file.seek(std::io::SeekFrom::Start(self.offset))?;
        let mut appended = Vec::new();
        file.read_to_end(&mut appended)?;
        self.offset = self.offset.saturating_add(appended.len() as u64);
        self.pending.extend_from_slice(&appended);

        let Some(complete_end) = self
            .pending
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
        else {
            return Ok(Vec::new());
        };
        let complete: Vec<u8> = self.pending.drain(..complete_end).collect();
        let events = complete
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_slice(line).unwrap_or_else(|error| {
                    AttachmentEvent::Notice(format!("skipped invalid attachment event: {error}"))
                })
            })
            .collect();
        Ok(events)
    }
}

struct AttachmentApp {
    id: String,
    directory: PathBuf,
    reader: AttachmentReader,
    transcript: Vec<Entry>,
    pending_tool: Option<ToolEntry>,
    context_usage: Option<ContextUsage>,
    usage: Option<ModelUsage>,
    provider_attempt_start: usize,
    status: Option<RunStatus>,
    reported_state: Option<RunState>,
    herdr: HerdrReporter,
    scroll_from_bottom: usize,
    viewport_height: usize,
    should_quit: bool,
}

impl AttachmentApp {
    fn new(id: &str, directory: PathBuf, herdr: HerdrReporter) -> Self {
        let reader = AttachmentReader::new(directory.join(subagent::ATTACHMENT_FILE_NAME));
        Self {
            id: id.to_string(),
            directory,
            reader,
            transcript: Vec::new(),
            pending_tool: None,
            context_usage: None,
            usage: None,
            provider_attempt_start: 0,
            status: None,
            reported_state: None,
            herdr,
            scroll_from_bottom: 0,
            viewport_height: 0,
            should_quit: false,
        }
    }

    async fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let mut terminal_events = TerminalEvents::new();
        let mut refresh = tokio::time::interval(REFRESH_INTERVAL);
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        self.refresh().await?;
        terminal.draw(|frame| self.draw(frame))?;

        while !self.should_quit {
            let redraw = tokio::select! {
                event = terminal_events.next() => self.handle_event(event?),
                _ = refresh.tick() => self.refresh().await?,
            };
            if redraw {
                terminal.draw(|frame| self.draw(frame))?;
            }
        }
        Ok(())
    }

    async fn refresh(&mut self) -> anyhow::Result<bool> {
        let events = self.reader.read_new()?;
        let mut changed = !events.is_empty();
        for event in events {
            self.apply_event(event);
        }
        let status_path = self.directory.join(subagent::RESULT_FILE_NAME);
        if let Some(status) = subagent::read_status(&status_path) {
            changed |= self.status.as_ref() != Some(&status);
            let state_changed = self.reported_state != Some(status.state);
            self.status = Some(status.clone());
            if state_changed {
                let (state, message) = herdr_status(&self.id, &status);
                self.herdr
                    .report_state(state, Some(&message), Some(&self.id))
                    .await;
                self.reported_state = Some(status.state);
            }
        }
        Ok(changed)
    }

    fn apply_event(&mut self, event: AttachmentEvent) {
        match event {
            AttachmentEvent::Prompt(prompt) => self.transcript.push(Entry::User(prompt)),
            AttachmentEvent::AssistantTextDelta(text) => {
                let can_append = self.provider_attempt_start < self.transcript.len();
                append_stream(
                    &mut self.transcript,
                    StreamTarget::Assistant,
                    text,
                    can_append,
                );
            }
            AttachmentEvent::ReasoningDelta(text) => {
                let can_append = self.provider_attempt_start < self.transcript.len();
                append_stream(
                    &mut self.transcript,
                    StreamTarget::Reasoning,
                    text,
                    can_append,
                );
            }
            AttachmentEvent::ToolStarted { display_lines }
            | AttachmentEvent::ToolUpdated { display_lines } => {
                self.pending_tool = Some(ToolEntry {
                    state: ToolEntryState::Running,
                    display_lines,
                    expanded: false,
                });
            }
            AttachmentEvent::ToolFinished {
                ok,
                display_style,
                display_lines,
            } => {
                self.pending_tool = None;
                self.transcript.push(Entry::Tool(ToolEntry {
                    state: ToolEntryState::Finished { ok, display_style },
                    display_lines,
                    expanded: false,
                }));
            }
            AttachmentEvent::Notice(notice) => self.transcript.push(Entry::Notice(notice)),
            AttachmentEvent::ContextUsage(usage) => self.context_usage = Some(usage),
            AttachmentEvent::Usage(usage) => self.usage = Some(usage),
            AttachmentEvent::StepStarted => {
                self.provider_attempt_start = self.transcript.len();
            }
            AttachmentEvent::ProviderStreamReset => {
                self.transcript.truncate(self.provider_attempt_start);
                self.pending_tool = None;
            }
            AttachmentEvent::Completed => {
                self.pending_tool = None;
            }
            AttachmentEvent::Cancelled => {
                self.pending_tool = None;
                self.transcript
                    .push(Entry::Notice("subagent stopped".into()));
            }
            AttachmentEvent::Failed(message) => {
                self.pending_tool = None;
                self.transcript.push(Entry::Error(message));
            }
        }
    }

    fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                    true
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    self.should_quit = true;
                    true
                }
                KeyCode::Up => {
                    self.scroll_up(1);
                    true
                }
                KeyCode::Down => {
                    self.scroll_down(1);
                    true
                }
                KeyCode::PageUp => {
                    self.scroll_up(self.viewport_height.max(1));
                    true
                }
                KeyCode::PageDown => {
                    self.scroll_down(self.viewport_height.max(1));
                    true
                }
                KeyCode::Home => {
                    self.scroll_from_bottom = usize::MAX;
                    true
                }
                KeyCode::End => {
                    self.scroll_from_bottom = 0;
                    true
                }
                _ => false,
            },
            Event::Resize(_, _) => true,
            _ => false,
        }
    }

    fn scroll_up(&mut self, lines: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lines);
    }

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
        let width = area.width as usize;
        let status = self.status.as_ref();
        let state = status.map_or("starting", |status| status.state.as_str());
        let preset = status
            .and_then(|status| status.preset.as_deref())
            .unwrap_or("subagent");
        let activity = status
            .and_then(|status| status.last_activity.as_deref())
            .unwrap_or("waiting for activity");
        let metrics = status.map_or_else(
            || format!("{preset}  |  {activity}"),
            |status| {
                format!(
                    "{preset}  |  {activity}  |  turn {}  |  tokens {}/{}",
                    status.turns, status.input_tokens, status.output_tokens
                )
            },
        );
        let mut live_metrics = Vec::new();
        if let Some(context) = &self.context_usage {
            let tokens = context
                .tokens
                .map_or_else(|| "?".into(), |tokens| tokens.to_string());
            let window = context
                .context_window
                .map_or_else(|| "?".into(), |tokens| tokens.to_string());
            live_metrics.push(format!("context {tokens}/{window}"));
        }
        if let Some(usage) = &self.usage {
            let input = usage
                .total_input_tokens()
                .map_or_else(|| "?".into(), |tokens| tokens.to_string());
            let output = usage
                .output_tokens
                .map_or_else(|| "?".into(), |tokens| tokens.to_string());
            live_metrics.push(format!("step tokens {input}/{output}"));
        }
        let header = vec![
            Line::from(vec![
                Span::styled("rho", Theme::brand()),
                Span::raw(format!("  attached to {}", self.id)),
                Span::styled(format!("  {state}"), state_style(status)),
            ]),
            Line::styled(truncate_one_line(&metrics, width), Theme::dim()),
            Line::styled(
                truncate_one_line(&live_metrics.join("  |  "), width),
                Theme::dim(),
            ),
            Line::styled("─".repeat(width.max(1)), Theme::dim()),
        ];
        frame.render_widget(Paragraph::new(header), chunks[0]);

        let mut lines = Vec::new();
        for entry in &self.transcript {
            lines.extend(entry_lines(entry, width, MAX_TOOL_OUTPUT_LINES));
        }
        if let Some(tool) = &self.pending_tool {
            lines.extend(entry_lines(
                &Entry::Tool(tool.clone()),
                width,
                MAX_TOOL_OUTPUT_LINES,
            ));
        }
        let has_assistant = self
            .transcript
            .iter()
            .any(|entry| matches!(entry, Entry::Assistant(_)));
        if !has_assistant {
            let fallback = status.and_then(|status| {
                status
                    .result
                    .as_deref()
                    .or(status.last_text.as_deref())
                    .filter(|text| !text.is_empty())
            });
            if let Some(text) = fallback {
                lines.extend(entry_lines(
                    &Entry::Assistant(text.to_string()),
                    width,
                    MAX_TOOL_OUTPUT_LINES,
                ));
            }
        }
        if let Some(error) = status.and_then(|status| status.error.as_deref()) {
            lines.extend(entry_lines(
                &Entry::Error(error.to_string()),
                width,
                MAX_TOOL_OUTPUT_LINES,
            ));
        }
        if let Some(error) = status.and_then(|status| status.attachment_error.as_deref()) {
            lines.extend(entry_lines(
                &Entry::Error(error.to_string()),
                width,
                MAX_TOOL_OUTPUT_LINES,
            ));
        }
        if lines.is_empty() {
            lines.push(Line::styled("waiting for subagent output...", Theme::dim()));
        }

        self.viewport_height = chunks[1].height as usize;
        let max_scroll = lines.len().saturating_sub(self.viewport_height);
        self.scroll_from_bottom = self.scroll_from_bottom.min(max_scroll);
        let start = max_scroll.saturating_sub(self.scroll_from_bottom);
        let end = start.saturating_add(self.viewport_height).min(lines.len());
        frame.render_widget(Paragraph::new(lines[start..end].to_vec()), chunks[1]);

        let footer = vec![
            Line::styled("─".repeat(width.max(1)), Theme::dim()),
            Line::styled(
                truncate_one_line(
                    "read-only  |  up/down scroll  |  home/end  |  q detach",
                    width,
                ),
                Theme::dim(),
            ),
        ];
        frame.render_widget(Paragraph::new(footer).style(Style::default()), chunks[2]);
    }
}

#[derive(Clone, Copy)]
enum StreamTarget {
    Assistant,
    Reasoning,
}

fn append_stream(
    transcript: &mut Vec<Entry>,
    target: StreamTarget,
    text: String,
    can_append: bool,
) {
    match (target, transcript.last_mut().filter(|_| can_append)) {
        (StreamTarget::Assistant, Some(Entry::Assistant(existing)))
        | (StreamTarget::Reasoning, Some(Entry::Reasoning(existing))) => existing.push_str(&text),
        (StreamTarget::Assistant, _) => transcript.push(Entry::Assistant(text)),
        (StreamTarget::Reasoning, _) => transcript.push(Entry::Reasoning(text)),
    }
}

fn herdr_status(id: &str, status: &RunStatus) -> (HerdrState, String) {
    let state = match status.state {
        RunState::Starting | RunState::Running => HerdrState::Working,
        RunState::Error => HerdrState::Blocked,
        RunState::Ok | RunState::Stopped => HerdrState::Idle,
    };
    let detail = status
        .last_activity
        .as_deref()
        .unwrap_or_else(|| status.state.as_str());
    (state, format!("subagent {id}: {detail}"))
}

fn state_style(status: Option<&RunStatus>) -> ratatui::style::Style {
    match status.map(|status| status.state) {
        Some(RunState::Ok) => Theme::success(),
        Some(RunState::Error | RunState::Stopped) => Theme::error(),
        Some(RunState::Starting | RunState::Running) | None => Theme::warning(),
    }
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;
