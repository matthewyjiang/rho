use std::{
    io::IsTerminal,
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use rho_sdk::model::{ContextUsage, ModelUsage};
use rho_tools::tool_card::ToolCard;

use crate::{
    herdr::{HerdrReporter, HerdrState},
    run_artifacts::{AttachmentEvent, AttachmentReader},
    subagent::{self, RunState, RunStatus},
};

use super::super::{
    feed_image::DEFAULT_IMAGE_HEIGHT,
    mouse_capture,
    provider_attempt::ProviderAttempt,
    render::{entry_lines, truncate_one_line},
    scrollbar::{HistoryScrollChrome, HistoryScrollbar, ScrollbarMouseInput},
    terminal_events::TerminalEvents,
    theme::Theme,
    usage_cost::{
        format_token_count, format_usage_token_summary, format_usd, resolved_usage_cost_usd_micros,
        AttemptAwareRunUsage,
    },
    Entry, HistoryScroll, ReasoningEntry, ToolEntry, HISTORY_MOUSE_SCROLL_LINES,
    HISTORY_SCROLLBAR_REVEAL_DURATION,
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_TOOL_OUTPUT_LINES: usize = 20;

pub(crate) async fn run(id: &str, herdr: HerdrReporter) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("rho attach requires an interactive terminal");
    }
    let id = subagent::normalize_id(id)?;
    let lookup_id = id.clone();
    let directory =
        tokio::task::spawn_blocking(move || subagent::resolve_run_directory(&lookup_id)).await??;

    let mut terminal = ratatui::init();
    let _restore_terminal = RestoreTerminal {
        mouse_capture: mouse_capture::Guard::acquire(),
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
    mouse_capture: mouse_capture::Guard,
}

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        // Disable mouse capture before leaving the alternate screen.
        self.mouse_capture.release();
        ratatui::restore();
    }
}

struct AttachmentApp {
    id: String,
    directory: PathBuf,
    reader: AttachmentReader,
    transcript: Vec<Entry>,
    /// Live tools keyed by call id (or a legacy singleton key for old journals).
    pending_tools: std::collections::BTreeMap<String, ToolEntry>,
    pending_order: Vec<String>,
    context_usage: Option<ContextUsage>,
    /// Latest provider usage for the attached run, including failed attempts.
    run_usage: AttemptAwareRunUsage,
    provider_attempt: ProviderAttempt,
    status: Option<RunStatus>,
    reported_state: Option<RunState>,
    /// Last whole-second live elapsed painted into the header (running runs only).
    last_drawn_elapsed_secs: Option<u64>,
    herdr: HerdrReporter,
    scroll: HistoryScrollChrome,
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
            pending_tools: std::collections::BTreeMap::new(),
            pending_order: Vec::new(),
            context_usage: None,
            run_usage: AttemptAwareRunUsage::default(),
            provider_attempt: ProviderAttempt::default(),
            status: None,
            reported_state: None,
            last_drawn_elapsed_secs: None,
            herdr,
            scroll: HistoryScrollChrome::default(),
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
        self.last_drawn_elapsed_secs = self.live_elapsed_secs();

        while !self.should_quit {
            let redraw = tokio::select! {
                event = terminal_events.next() => self.handle_event(event?),
                _ = refresh.tick() => {
                    let changed = self.refresh().await?;
                    // Keep redrawing while the auto-hide scrollbar is visible, and
                    // when a live run's whole-second elapsed label advances without I/O.
                    changed
                        || self.scroll.should_render(Instant::now())
                        || self.live_elapsed_secs() != self.last_drawn_elapsed_secs
                },
            };
            if redraw {
                terminal.draw(|frame| self.draw(frame))?;
                self.last_drawn_elapsed_secs = self.live_elapsed_secs();
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

    /// Whole-second live elapsed for non-terminal runs with `started_at`.
    fn live_elapsed_secs(&self) -> Option<u64> {
        let status = self.status.as_ref()?;
        if status.state.is_terminal() {
            return None;
        }
        status
            .elapsed_duration(subagent::unix_now_secs())
            .map(|elapsed| elapsed.as_secs())
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
            AttachmentEvent::ToolStarted { key, card }
            | AttachmentEvent::ToolUpdated { key, card } => {
                self.upsert_pending_tool(key, card);
            }
            AttachmentEvent::ToolFinished { key, card } => {
                self.finish_pending_tool(key, card);
            }
            AttachmentEvent::Notice(notice) => self.transcript.push(Entry::Notice(notice)),
            AttachmentEvent::ContextUsage(usage) => self.context_usage = Some(usage),
            AttachmentEvent::Usage(usage) => {
                self.run_usage.apply_snapshot(usage, |snapshot| snapshot);
            }
            AttachmentEvent::StepStarted => {
                self.provider_attempt.begin(self.transcript.len());
                self.run_usage.step_started();
            }
            AttachmentEvent::ProviderStreamReset => {
                self.provider_attempt.reset_output(&mut self.transcript);
                self.clear_pending_tools();
                self.run_usage.attempt_reset();
            }
            AttachmentEvent::Completed => {
                self.clear_pending_tools();
            }
            AttachmentEvent::Cancelled => {
                self.clear_pending_tools();
                self.transcript.push(Entry::Notice("agent stopped".into()));
            }
            AttachmentEvent::Failed(message) => {
                self.clear_pending_tools();
                self.transcript.push(Entry::Error(message));
            }
        }
    }

    fn upsert_pending_tool(&mut self, key: Option<String>, card: ToolCard) {
        let key = attachment_tool_key(key);
        let expanded = self
            .pending_tools
            .get(&key)
            .is_some_and(|entry| entry.expanded);
        if !self.pending_tools.contains_key(&key) {
            self.pending_order.push(key.clone());
        }
        self.pending_tools.insert(
            key,
            ToolEntry {
                card,
                expanded,
                image: None,
            },
        );
    }

    fn finish_pending_tool(&mut self, key: Option<String>, card: ToolCard) {
        let key = attachment_tool_key(key);
        self.pending_tools.remove(&key);
        self.pending_order.retain(|pending| pending != &key);
        self.transcript.push(Entry::Tool(ToolEntry {
            card,
            expanded: false,
            image: None,
        }));
    }

    fn clear_pending_tools(&mut self) {
        self.pending_tools.clear();
        self.pending_order.clear();
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
                    self.scroll
                        .set_top_line(self.content_len, self.viewport_height, 0);
                    if !matches!(self.scroll.scroll(), HistoryScroll::Bottom) {
                        self.scroll.reveal(now, HISTORY_SCROLLBAR_REVEAL_DURATION);
                    }
                    true
                }
                KeyCode::End => {
                    self.scroll.scroll_to_bottom();
                    true
                }
                _ => false,
            },
            Event::Mouse(mouse) => {
                if matches!(mouse.kind, MouseEventKind::Moved)
                    && self.last_mouse_position == Some((mouse.column, mouse.row))
                {
                    return false;
                }
                if matches!(mouse.kind, MouseEventKind::Moved) {
                    self.last_mouse_position = Some((mouse.column, mouse.row));
                }
                self.scroll.handle_scrollbar_mouse(
                    mouse.kind,
                    mouse.column,
                    mouse.row,
                    ScrollbarMouseInput {
                        now,
                        reveal_duration: HISTORY_SCROLLBAR_REVEAL_DURATION,
                        scrollbar: self.history_scrollbar(),
                        content_len: self.content_len,
                        viewport_len: self.viewport_height,
                        wheel_lines: HISTORY_MOUSE_SCROLL_LINES,
                    },
                );
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

    fn scroll_lines(&mut self, now: Instant, delta: isize) {
        self.scroll
            .scroll_by(self.content_len, self.viewport_height, delta);
        if !matches!(self.scroll.scroll(), HistoryScroll::Bottom) {
            self.scroll.reveal(now, HISTORY_SCROLLBAR_REVEAL_DURATION);
        }
    }

    fn history_scrollbar(&self) -> Option<HistoryScrollbar> {
        HistoryScrollbar::new(
            self.history_area,
            self.content_len,
            self.scroll
                .visible_start(self.content_len, self.viewport_height),
        )
    }

    fn sync_history_geometry(&mut self, area: Rect, content_len: usize) {
        self.history_area = area;
        self.viewport_height = area.height as usize;
        self.content_len = content_len;
        self.scroll.clamp(self.content_len, self.viewport_height);
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
        let identity = identity_line(status, self.run_usage.current(), subagent::unix_now_secs());
        let activity_metrics = activity_metrics_line(
            activity,
            self.context_usage.as_ref(),
            self.run_usage.current(),
            status,
        );
        let header = vec![
            header_title_line(&self.id, agent_id, state, status),
            Line::styled(truncate_one_line(&identity, width), Theme::dim()),
            Line::styled(truncate_one_line(&activity_metrics, width), Theme::dim()),
            Line::styled("─".repeat(width.max(1)), Theme::dim()),
        ];
        frame.render_widget(Paragraph::new(header), chunks[0]);

        let lines = self.history_lines(width, status);
        self.sync_history_geometry(chunks[1], lines.len());
        let start = self
            .scroll
            .visible_start(self.content_len, self.viewport_height);
        let end = start.saturating_add(self.viewport_height).min(lines.len());
        frame.render_widget(Paragraph::new(lines[start..end].to_vec()), chunks[1]);

        let now = Instant::now();
        if let Some(scrollbar) = self
            .history_scrollbar()
            .filter(|_| self.scroll.should_render(now))
        {
            scrollbar.render(frame, self.scroll.drag().is_some());
        }

        let footer = vec![
            Line::styled("─".repeat(width.max(1)), Theme::dim()),
            Line::styled(
                truncate_one_line("read-only · scroll · home/end · q detach", width),
                Theme::dim(),
            ),
        ];
        frame.render_widget(Paragraph::new(footer).style(Style::default()), chunks[2]);
    }

    fn history_lines(&self, width: usize, status: Option<&RunStatus>) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for entry in &self.transcript {
            lines.extend(entry_lines(
                entry,
                width,
                MAX_TOOL_OUTPUT_LINES,
                DEFAULT_IMAGE_HEIGHT,
            ));
        }
        for key in &self.pending_order {
            if let Some(tool) = self.pending_tools.get(key) {
                lines.extend(entry_lines(
                    &Entry::Tool(tool.clone()),
                    width,
                    MAX_TOOL_OUTPUT_LINES,
                    DEFAULT_IMAGE_HEIGHT,
                ));
            }
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
                    DEFAULT_IMAGE_HEIGHT,
                ));
            }
        }
        if let Some(error) = status.and_then(|status| status.error.as_deref()) {
            lines.extend(entry_lines(
                &Entry::Error(error.to_string()),
                width,
                MAX_TOOL_OUTPUT_LINES,
                DEFAULT_IMAGE_HEIGHT,
            ));
        }
        if let Some(error) = status.and_then(|status| status.attachment_error.as_deref()) {
            lines.extend(entry_lines(
                &Entry::Error(error.to_string()),
                width,
                MAX_TOOL_OUTPUT_LINES,
                DEFAULT_IMAGE_HEIGHT,
            ));
        }
        if lines.is_empty() {
            lines.push(Line::styled("waiting for agent output...", Theme::dim()));
        }
        lines
    }
}

/// Middle header row: model, runtime, turn, elapsed, optional Claude session, cost.
fn identity_line(
    status: Option<&RunStatus>,
    run_usage: Option<&ModelUsage>,
    now_unix_secs: u64,
) -> String {
    let Some(status) = status else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(model) = format_model_identity(status) {
        parts.push(model);
    }
    if let Some(runtime) = status.runtime {
        parts.push(runtime.as_str().to_string());
    }
    parts.push(format!("turn {}", status.turns));
    if let Some(elapsed) = status
        .elapsed_duration(now_unix_secs)
        .map(|elapsed| subagent::format_elapsed_secs(elapsed.as_secs()))
    {
        parts.push(elapsed);
    }
    if let Some(session_id) = status
        .claude_session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
    {
        parts.push(format!("claude {session_id}"));
    }
    if let Some(cost) = format_run_cost(status, run_usage) {
        parts.push(cost);
    }
    join_fields(parts)
}

/// Bottom header row: what the run is doing plus live usage.
fn activity_metrics_line(
    activity: &str,
    context: Option<&ContextUsage>,
    run_usage: Option<&ModelUsage>,
    status: Option<&RunStatus>,
) -> String {
    let mut parts = vec![activity.to_string()];
    parts.extend(usage_metric_parts(context, run_usage, status));
    join_fields(parts)
}

fn usage_metric_parts(
    context: Option<&ContextUsage>,
    run_usage: Option<&ModelUsage>,
    status: Option<&RunStatus>,
) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(context_summary) = format_context_summary(context) {
        parts.push(context_summary);
    }
    if let Some(usage_summary) = run_usage
        .and_then(format_usage_token_summary)
        .or_else(|| status.and_then(|status| format_usage_token_summary(&run_status_usage(status))))
    {
        parts.push(usage_summary);
    }
    parts
}

fn header_title_line(
    run_id: &str,
    agent_id: &str,
    state: &str,
    status: Option<&RunStatus>,
) -> Line<'static> {
    Line::from(vec![
        Span::styled("rho", Theme::brand()),
        Span::raw(format!("  attach {run_id}")),
        Span::styled(format!(" · {agent_id}"), Theme::dim()),
        Span::styled(format!(" · {state}"), state_style(status)),
    ])
}

fn format_model_identity(status: &RunStatus) -> Option<String> {
    let provider = status
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let model = status
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (provider, model) {
        (Some(provider), Some(model)) => {
            Some(rho_providers::provider::model_reference(provider, model))
        }
        (None, Some(model)) => Some(model.to_string()),
        (Some(provider), None) => Some(provider.to_string()),
        (None, None) => None,
    }
}

fn join_fields(parts: Vec<String>) -> String {
    parts.join(FIELD_SEP)
}

/// Separator between attach header fields. Matches the main TUI statusline.
const FIELD_SEP: &str = " · ";

fn run_status_usage(status: &RunStatus) -> ModelUsage {
    ModelUsage {
        input_tokens: status.input_tokens,
        output_tokens: status.output_tokens,
        ..ModelUsage::default()
    }
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

fn format_run_cost(status: &RunStatus, run_usage: Option<&ModelUsage>) -> Option<String> {
    if let Some(cost) = status.total_cost_usd {
        return Some(format_usd(subagent::usd_to_micros(cost)));
    }
    // Attach has no model metadata, so this resolves provider-reported cost only.
    run_usage
        .and_then(|usage| resolved_usage_cost_usd_micros(usage, None))
        .map(format_usd)
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

/// Map optional journal keys onto a stable pending-tool slot.
///
/// Legacy unkeyed events share one singleton slot so old journals keep working.
fn attachment_tool_key(key: Option<String>) -> String {
    key.filter(|key| !key.is_empty())
        .unwrap_or_else(|| "__legacy__".into())
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
