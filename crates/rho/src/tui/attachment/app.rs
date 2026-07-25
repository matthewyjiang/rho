use std::{
    io::IsTerminal,
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use rho_sdk::model::{ContextUsage, ModelUsage};

use crate::{
    herdr::{HerdrReporter, HerdrState},
    run_artifacts::{AttachmentEvent, AttachmentReader},
    subagent::{self, RunState, RunStatus},
};

use super::super::{
    mouse_capture,
    provider_attempt::ProviderAttempt,
    render::{entry_lines, truncate_one_line},
    scrollbar::{scroll_state_for_top_line, HistoryScrollbar, HistoryScrollbarDrag},
    terminal_events::TerminalEvents,
    theme::Theme,
    usage_cost::{format_usd, merge_usage, usage_difference},
    Entry, HistoryScroll, ReasoningEntry, ToolEntry, ToolEntryState,
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_TOOL_OUTPUT_LINES: usize = 20;
const SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(1200);
const MOUSE_SCROLL_LINES: usize = 3;

pub(crate) async fn run(id: &str, herdr: HerdrReporter) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("rho attach requires an interactive terminal");
    }
    let id = subagent::normalize_id(id)?;
    let directory = subagent::directory(&id)?;
    if !directory.is_dir() {
        anyhow::bail!("unknown delegated run '{id}'");
    }
    subagent::secure_directory(&directory)?;

    let mut terminal = ratatui::init();
    let _restore_terminal = RestoreTerminal {
        mouse_capture_enabled: mouse_capture::enable().is_ok(),
    };
    Theme::initialize_from_terminal();
    let message = format!("attached to agent run {id}");
    herdr
        .report_state(HerdrState::Working, Some(&message), Some(&id))
        .await;
    let result = AttachmentApp::new(&id, directory, herdr.clone())
        .run(&mut terminal)
        .await;
    herdr.release().await;
    result
}

struct RestoreTerminal {
    mouse_capture_enabled: bool,
}

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        if self.mouse_capture_enabled {
            let _ = mouse_capture::disable();
        }
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
    /// Accumulated usage for the attached run, including failed provider attempts.
    run_usage: Option<ModelUsage>,
    /// Usage committed before the current provider attempt (retry baseline).
    usage_before_current_attempt: Option<ModelUsage>,
    /// Usage committed before the current step (for last-step deltas).
    usage_before_current_step: Option<ModelUsage>,
    provider_attempt: ProviderAttempt,
    status: Option<RunStatus>,
    reported_state: Option<RunState>,
    herdr: HerdrReporter,
    scroll: HistoryScroll,
    scrollbar_drag: Option<HistoryScrollbarDrag>,
    scrollbar_visible_until: Option<Instant>,
    scrollbar_hovered: bool,
    last_mouse_position: Option<(u16, u16)>,
    viewport_height: usize,
    history_area: Rect,
    content_len: usize,
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
            run_usage: None,
            usage_before_current_attempt: None,
            usage_before_current_step: None,
            provider_attempt: ProviderAttempt::default(),
            status: None,
            reported_state: None,
            herdr,
            scroll: HistoryScroll::Bottom,
            scrollbar_drag: None,
            scrollbar_visible_until: None,
            scrollbar_hovered: false,
            last_mouse_position: None,
            viewport_height: 0,
            history_area: Rect::default(),
            content_len: 0,
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
                _ = refresh.tick() => {
                    let changed = self.refresh().await?;
                    // Keep redrawing while the auto-hide scrollbar is visible.
                    changed || self.should_render_scrollbar(Instant::now())
                },
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
            AttachmentEvent::Usage(usage) => self.record_usage(usage),
            AttachmentEvent::StepStarted => {
                self.provider_attempt.begin(self.transcript.len());
                self.usage_before_current_step = self.run_usage.clone();
                self.usage_before_current_attempt = None;
            }
            AttachmentEvent::ProviderStreamReset => {
                self.provider_attempt.reset_output(&mut self.transcript);
                self.pending_tool = None;
                // Keep tokens already charged on the failed attempt, then accept
                // the next usage snapshot as a fresh attempt total.
                self.usage_before_current_attempt = self
                    .run_usage
                    .as_ref()
                    .map(|usage| usage_difference(usage, self.usage_before_current_step.as_ref()));
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

    fn record_usage(&mut self, usage: ModelUsage) {
        // Provider usage snapshots are cumulative within the current attempt.
        // Merge any prior failed-attempt tokens so retries do not drop cost.
        let mut current_run_usage = usage;
        if let Some(attempt_baseline) = &self.usage_before_current_attempt {
            let mut combined = None;
            merge_usage(&mut combined, attempt_baseline.clone());
            merge_usage(&mut combined, current_run_usage);
            current_run_usage = combined.expect("attempt baseline is present");
        }
        self.run_usage = Some(current_run_usage);
    }

    fn handle_event(&mut self, event: Event) -> bool {
        let now = Instant::now();
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
                    self.scroll_lines(now, -1);
                    true
                }
                KeyCode::Down => {
                    self.scroll_lines(now, 1);
                    true
                }
                KeyCode::PageUp => {
                    self.scroll_lines(now, -(self.viewport_height.max(1) as isize));
                    true
                }
                KeyCode::PageDown => {
                    self.scroll_lines(now, self.viewport_height.max(1) as isize);
                    true
                }
                KeyCode::Home => {
                    self.reveal_scrollbar(now);
                    self.scrollbar_drag = None;
                    self.scroll =
                        scroll_state_for_top_line(self.content_len, self.viewport_height, 0);
                    true
                }
                KeyCode::End => {
                    self.scroll_to_bottom();
                    true
                }
                _ => false,
            },
            Event::Mouse(mouse) => {
                self.handle_mouse(mouse.kind, mouse.column, mouse.row, now);
                true
            }
            Event::FocusGained => {
                mouse_capture::reassert();
                false
            }
            Event::Resize(_, _) => true,
            _ => false,
        }
    }

    fn handle_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16, now: Instant) {
        match kind {
            MouseEventKind::ScrollUp => {
                self.reveal_scrollbar(now);
                self.scrollbar_drag = None;
                self.scroll_lines(now, -(MOUSE_SCROLL_LINES as isize));
            }
            MouseEventKind::ScrollDown => {
                self.reveal_scrollbar(now);
                self.scrollbar_drag = None;
                self.scroll_lines(now, MOUSE_SCROLL_LINES as isize);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let scrollbar = self
                    .history_scrollbar()
                    .filter(|scrollbar| scrollbar.contains(column, row))
                    .filter(|_| self.should_render_scrollbar(now));
                self.update_scrollbar_hover(column, row);
                if let Some(scrollbar) = scrollbar {
                    self.reveal_scrollbar(now);
                    let drag = scrollbar.begin_drag(row);
                    self.scrollbar_drag = Some(drag);
                    self.scroll = scrollbar.scroll_state_for_pointer(row, drag);
                } else {
                    self.scrollbar_drag = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.update_scrollbar_hover(column, row);
                if let (Some(drag), Some(scrollbar)) =
                    (self.scrollbar_drag, self.history_scrollbar())
                {
                    self.scroll = scrollbar.scroll_state_for_pointer(row, drag);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.scrollbar_drag = None;
                self.update_scrollbar_hover(column, row);
            }
            MouseEventKind::Moved if self.last_mouse_position == Some((column, row)) => {}
            MouseEventKind::Moved => {
                self.last_mouse_position = Some((column, row));
                self.update_scrollbar_hover(column, row);
            }
            MouseEventKind::Down(MouseButton::Right)
            | MouseEventKind::Down(MouseButton::Middle)
            | MouseEventKind::Up(MouseButton::Right)
            | MouseEventKind::Up(MouseButton::Middle)
            | MouseEventKind::Drag(MouseButton::Right)
            | MouseEventKind::Drag(MouseButton::Middle)
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {}
        }
    }

    fn scroll_lines(&mut self, now: Instant, delta: isize) {
        let max_start = self.content_len.saturating_sub(self.viewport_height);
        let current = self.visible_start();
        let next = current.saturating_add_signed(delta).min(max_start);
        self.scroll = scroll_state_for_top_line(self.content_len, self.viewport_height, next);
        if matches!(self.scroll, HistoryScroll::Bottom) {
            self.hide_scrollbar();
        } else {
            self.reveal_scrollbar(now);
            self.scrollbar_drag = None;
        }
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll = HistoryScroll::Bottom;
        self.hide_scrollbar();
    }

    fn visible_start(&self) -> usize {
        let max_start = self.content_len.saturating_sub(self.viewport_height);
        match self.scroll {
            HistoryScroll::Bottom => max_start,
            HistoryScroll::Manual { top_line } => top_line.min(max_start),
        }
    }

    fn history_scrollbar(&self) -> Option<HistoryScrollbar> {
        HistoryScrollbar::new(self.history_area, self.content_len, self.visible_start())
    }

    fn reveal_scrollbar(&mut self, now: Instant) {
        self.scrollbar_visible_until = Some(now + SCROLLBAR_REVEAL_DURATION);
    }

    fn hide_scrollbar(&mut self) {
        self.scrollbar_drag = None;
        self.scrollbar_visible_until = None;
        self.scrollbar_hovered = false;
    }

    fn should_render_scrollbar(&self, now: Instant) -> bool {
        self.scrollbar_drag.is_some()
            || self.scrollbar_hovered
            || self
                .scrollbar_visible_until
                .is_some_and(|visible_until| now < visible_until)
    }

    fn update_scrollbar_hover(&mut self, column: u16, row: u16) {
        self.scrollbar_hovered = self
            .history_scrollbar()
            .is_some_and(|scrollbar| scrollbar.contains(column, row));
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
        let metrics = status_metrics_line(status, agent_id, activity, self.run_usage.as_ref());
        let live_metrics =
            live_metrics_line(self.context_usage.as_ref(), self.run_usage.as_ref(), status);
        let header = vec![
            Line::from(vec![
                Span::styled("rho", Theme::brand()),
                Span::raw(format!("  attached to {}", self.id)),
                Span::styled(format!("  {state}"), state_style(status)),
            ]),
            Line::styled(truncate_one_line(&metrics, width), Theme::dim()),
            Line::styled(truncate_one_line(&live_metrics, width), Theme::dim()),
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

        self.history_area = chunks[1];
        self.viewport_height = chunks[1].height as usize;
        self.content_len = lines.len();
        if let HistoryScroll::Manual { top_line } = self.scroll {
            self.scroll =
                scroll_state_for_top_line(self.content_len, self.viewport_height, top_line);
            if matches!(self.scroll, HistoryScroll::Bottom) {
                self.hide_scrollbar();
            }
        }
        let start = self.visible_start();
        let end = start.saturating_add(self.viewport_height).min(lines.len());
        frame.render_widget(Paragraph::new(lines[start..end].to_vec()), chunks[1]);

        let now = Instant::now();
        if let Some(scrollbar) = self
            .history_scrollbar()
            .filter(|_| self.should_render_scrollbar(now))
        {
            scrollbar.render(frame, self.scrollbar_drag.is_some());
        }

        let footer = vec![
            Line::styled("─".repeat(width.max(1)), Theme::dim()),
            Line::styled(
                truncate_one_line("read-only  |  scroll  |  home/end  |  q detach", width),
                Theme::dim(),
            ),
        ];
        frame.render_widget(Paragraph::new(footer).style(Style::default()), chunks[2]);
    }
}

fn status_metrics_line(
    status: Option<&RunStatus>,
    agent_id: &str,
    activity: &str,
    run_usage: Option<&ModelUsage>,
) -> String {
    let Some(status) = status else {
        return format!("{agent_id}  |  {activity}");
    };
    let mut parts = vec![
        agent_id.to_string(),
        activity.to_string(),
        format!("turn {}", status.turns),
    ];
    if let Some(session_id) = status.claude_session_id.as_deref() {
        parts.push(format!("claude {session_id}"));
    }
    if let Some(cost) = format_run_cost(status, run_usage) {
        parts.push(cost);
    }
    parts.join("  |  ")
}

fn live_metrics_line(
    context: Option<&ContextUsage>,
    run_usage: Option<&ModelUsage>,
    status: Option<&RunStatus>,
) -> String {
    let mut parts = Vec::new();
    if let Some(context_summary) = format_context_summary(context) {
        parts.push(context_summary);
    }
    if let Some(usage_summary) = run_usage
        .and_then(format_usage_summary)
        .or_else(|| status.and_then(format_status_token_summary))
    {
        parts.push(usage_summary);
    }
    parts.join("  |  ")
}

fn format_status_token_summary(status: &RunStatus) -> Option<String> {
    let mut parts = Vec::new();
    push_token_part(&mut parts, "in", status.input_tokens);
    push_token_part(&mut parts, "out", status.output_tokens);
    (!parts.is_empty()).then(|| format!("tokens {}", parts.join(" · ")))
}

fn format_context_summary(context: Option<&ContextUsage>) -> Option<String> {
    let context = context?;
    let tokens = context.tokens?;
    match context.context_window.filter(|window| *window > 0) {
        Some(window) => {
            let percent = tokens as f64 * 100.0 / window as f64;
            Some(format!(
                "context {}/{} ({percent:.1}%)",
                format_token_count(tokens),
                format_token_count(window)
            ))
        }
        None => Some(format!("context {}", format_token_count(tokens))),
    }
}

fn format_usage_summary(usage: &ModelUsage) -> Option<String> {
    let mut parts = Vec::new();
    push_token_part(&mut parts, "in", usage.input_tokens);
    push_token_part(&mut parts, "out", usage.output_tokens);
    push_token_part(&mut parts, "cache r", usage.cache_read_tokens);
    push_token_part(&mut parts, "cache w", usage.cache_write_tokens);
    if parts.is_empty() {
        // Fall back to total input when providers only report an aggregate.
        push_token_part(&mut parts, "in", usage.total_input_tokens());
        push_token_part(&mut parts, "out", usage.output_tokens);
    }
    (!parts.is_empty()).then(|| format!("tokens {}", parts.join(" · ")))
}

fn push_token_part(parts: &mut Vec<String>, label: &str, tokens: Option<u64>) {
    if let Some(tokens) = tokens {
        parts.push(format!("{label} {}", format_token_count(tokens)));
    }
}

fn format_run_cost(status: &RunStatus, run_usage: Option<&ModelUsage>) -> Option<String> {
    if let Some(cost) = status.total_cost_usd {
        return Some(format!("${cost:.4}"));
    }
    run_usage
        .and_then(|usage| usage.cost_usd_micros)
        .map(format_usd)
}

fn format_token_count(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
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
