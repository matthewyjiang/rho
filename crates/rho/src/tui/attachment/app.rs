use std::{io::IsTerminal, path::PathBuf, time::Duration};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use rho_sdk::model::{ContextUsage, ModelUsage};

use crate::{
    herdr::{HerdrReporter, HerdrState},
    subagent::{self, RunState, RunStatus},
};

use super::{
    super::{
        provider_attempt::ProviderAttempt,
        render::{entry_lines, truncate_one_line},
        terminal_events::TerminalEvents,
        theme::Theme,
        Entry, ReasoningEntry, ToolEntry, ToolEntryState,
    },
    journal::{AttachmentEvent, AttachmentReader},
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_TOOL_OUTPUT_LINES: usize = 20;

pub(crate) async fn run(id: &str, herdr: HerdrReporter) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("rho attach requires an interactive terminal");
    }
    let directory = subagent::directory(id)?;
    if !directory.is_dir() {
        anyhow::bail!("unknown delegated run '{id}'");
    }
    subagent::secure_directory(&directory)?;

    let mut terminal = ratatui::init();
    let _restore_terminal = RestoreTerminal;
    Theme::initialize_from_terminal();
    let message = format!("attached to agent run {id}");
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

struct AttachmentApp {
    id: String,
    directory: PathBuf,
    reader: AttachmentReader,
    transcript: Vec<Entry>,
    pending_tool: Option<ToolEntry>,
    context_usage: Option<ContextUsage>,
    usage: Option<ModelUsage>,
    provider_attempt: ProviderAttempt,
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
            provider_attempt: ProviderAttempt::default(),
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
                let can_append = self
                    .provider_attempt
                    .can_append_to_last(self.transcript.len());
                append_stream(
                    &mut self.transcript,
                    StreamTarget::Assistant,
                    text,
                    can_append,
                );
            }
            AttachmentEvent::ReasoningDelta(text) => {
                let can_append = self
                    .provider_attempt
                    .can_append_to_last(self.transcript.len());
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
                    image: None,
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
                    image: None,
                }));
            }
            AttachmentEvent::Notice(notice) => self.transcript.push(Entry::Notice(notice)),
            AttachmentEvent::ContextUsage(usage) => self.context_usage = Some(usage),
            AttachmentEvent::Usage(usage) => self.usage = Some(usage),
            AttachmentEvent::StepStarted => {
                self.provider_attempt.begin(self.transcript.len());
            }
            AttachmentEvent::ProviderStreamReset => {
                self.provider_attempt.reset_output(&mut self.transcript);
                self.pending_tool = None;
            }
            AttachmentEvent::Completed => {
                self.pending_tool = None;
            }
            AttachmentEvent::Cancelled => {
                self.pending_tool = None;
                self.transcript.push(Entry::Notice("agent stopped".into()));
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
        let agent_id = status
            .and_then(|status| status.agent_id.as_deref())
            .unwrap_or("agent");
        let activity = status
            .and_then(|status| status.last_activity.as_deref())
            .unwrap_or("waiting for activity");
        let metrics = status.map_or_else(
            || format!("{agent_id}  |  {activity}"),
            |status| {
                format!(
                    "{agent_id}  |  {activity}  |  turn {}  |  tokens {}/{}",
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
            lines.push(Line::styled("waiting for agent output...", Theme::dim()));
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
        (StreamTarget::Assistant, Some(Entry::Assistant(existing))) => existing.push_str(&text),
        (StreamTarget::Reasoning, Some(Entry::Reasoning(existing)))
            if existing.thought_for.is_none() =>
        {
            existing.text.push_str(&text)
        }
        (StreamTarget::Assistant, _) => transcript.push(Entry::Assistant(text)),
        (StreamTarget::Reasoning, _) => {
            transcript.push(Entry::Reasoning(ReasoningEntry::new(text)))
        }
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
    (state, format!("agent run {id}: {detail}"))
}

fn state_style(status: Option<&RunStatus>) -> ratatui::style::Style {
    match status.map(|status| status.state) {
        Some(RunState::Ok) => Theme::success(),
        Some(RunState::Error | RunState::Stopped) => Theme::error(),
        Some(RunState::Starting | RunState::Running) | None => Theme::warning(),
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
