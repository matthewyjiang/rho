use std::{
    collections::BTreeMap,
    io::IsTerminal,
    ops::Range,
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
use rho_tools::tool_card::ToolCard;

use crate::{
    herdr::{HerdrReporter, HerdrState},
    run_artifacts::{AttachmentEvent, AttachmentReader},
    subagent::{self, RunState, RunStatus},
};

use super::super::{
    live_started_at, mouse_capture,
    provider_attempt::ProviderAttempt,
    render::truncate_one_line,
    scrollbar::{HistoryScrollChrome, HistoryScrollbar, ScrollbarMouseInput},
    terminal_events::TerminalEvents,
    theme::Theme,
    tool_card_hover,
    usage_cost::{
        format_token_count, format_usage_token_summary, format_usd, resolved_usage_cost_usd_micros,
        AttemptAwareRunUsage,
    },
    Entry, HistoryScroll, ReasoningChrome, ReasoningEntry, ToolEntry, HISTORY_MOUSE_SCROLL_LINES,
    HISTORY_SCROLLBAR_REVEAL_DURATION,
};
use super::tool_toggle::{
    latest_toggle_target, status_fallback_items, tool_card_at_line, HistoryItem, ToggleTarget,
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const TOGGLE_WIDTH_FALLBACK: usize = 80;

/// Display policy for `rho attach`, mirrored from interactive config.
///
/// Keep ingest complete (including hidden reasoning) so journal replay and
/// provider-attempt bookkeeping stay stable; filter at render time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttachmentDisplaySettings {
    pub show_reasoning_output: bool,
    pub zen_mode: bool,
    pub max_tool_output_lines: usize,
    pub theme: String,
}

impl Default for AttachmentDisplaySettings {
    fn default() -> Self {
        Self::from_config(&crate::config::Config::default())
    }
}

impl AttachmentDisplaySettings {
    pub(crate) fn from_config(config: &crate::config::Config) -> Self {
        Self {
            show_reasoning_output: config.show_reasoning_output,
            zen_mode: config.zen_mode,
            max_tool_output_lines: config.max_tool_output_lines.max(1),
            theme: config.theme.clone(),
        }
    }

    /// Exclusive reasoning display policy; matches interactive TUI chrome.
    ///
    /// Attach has no live `Thinking...` stretch, so
    /// [`ReasoningChrome::ThinkingPlaceholder`] hides reasoning text the same
    /// way as [`ReasoningChrome::Hidden`]. Full text still requires
    /// `show_reasoning_output` with zen off.
    fn reasoning_chrome(&self) -> ReasoningChrome {
        if self.zen_mode {
            ReasoningChrome::Hidden
        } else if self.show_reasoning_output {
            ReasoningChrome::FullText
        } else {
            ReasoningChrome::ThinkingPlaceholder
        }
    }

    fn displays_reasoning_output(&self) -> bool {
        matches!(self.reasoning_chrome(), ReasoningChrome::FullText)
    }

    /// Tool cards stay visible outside zen mode.
    fn shows_work_chrome(&self) -> bool {
        !self.zen_mode
    }

    fn max_tool_output_lines(&self) -> usize {
        self.max_tool_output_lines.max(1)
    }

    /// Zen hides tools and reasoning. Hide-reasoning alone suppresses reasoning text.
    fn hides_entry(&self, entry: &Entry) -> bool {
        match entry {
            Entry::Reasoning(_) => !self.displays_reasoning_output(),
            Entry::Tool(_) => !self.shows_work_chrome(),
            _ => false,
        }
    }
}

pub(crate) async fn run(
    id: Option<&str>,
    display: AttachmentDisplaySettings,
    herdr: HerdrReporter,
) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("rho attach requires an interactive terminal");
    }
    let mut terminal = ratatui::init();
    let _restore_terminal = RestoreTerminal {
        mouse_capture: mouse_capture::Guard::acquire(),
    };
    Theme::initialize_from_terminal();
    Theme::apply_committed(&display.theme);
    let id = match id {
        Some(id) => subagent::normalize_id(id)?,
        None => match super::select::select_running_run(&mut terminal).await? {
            Some(id) => id,
            None => return Ok(()),
        },
    };
    let lookup_id = id.clone();
    let directory =
        tokio::task::spawn_blocking(move || subagent::resolve_run_directory(&lookup_id)).await??;
    let message = format!("attached to agent run {id}");
    herdr
        .report_state(HerdrState::Working, Some(&message), Some(&id))
        .await;
    let result = AttachmentApp::new(&id, directory, display, herdr.clone())
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
    display: AttachmentDisplaySettings,
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
    /// Stable tool key under the last left-button press, if any. Survives
    /// pending→transcript promotion and provider resets that shift indexes.
    press_tool_key: Option<String>,
    press_cell: Option<(u16, u16)>,
    /// Current transcript index for each finished tool key.
    finished_tool_index: BTreeMap<String, usize>,
    viewport_height: usize,
    history_area: Rect,
    history_width: usize,
    content_len: usize,
    should_quit: bool,
}

impl AttachmentApp {
    fn new(
        id: &str,
        directory: PathBuf,
        display: AttachmentDisplaySettings,
        herdr: HerdrReporter,
    ) -> Self {
        let reader = AttachmentReader::new(directory.join(subagent::ATTACHMENT_FILE_NAME));
        Self {
            id: id.to_string(),
            directory,
            reader,
            display,
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
            press_tool_key: None,
            press_cell: None,
            finished_tool_index: BTreeMap::new(),
            viewport_height: 0,
            history_area: Rect::default(),
            history_width: 0,
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
                self.reindex_finished_tools();
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
        let previous = self.pending_tools.get(&key);
        let expanded = previous.is_some_and(|entry| entry.expanded);
        let started_at = live_started_at(previous, card.status);
        if !self.pending_tools.contains_key(&key) {
            self.pending_order.push(key.clone());
        }
        self.pending_tools.insert(
            key,
            ToolEntry {
                card,
                expanded,
                image: None,
                started_at,
            },
        );
    }

    fn finish_pending_tool(&mut self, key: Option<String>, card: ToolCard) {
        let key = attachment_tool_key(key);
        let expanded = self
            .pending_tools
            .remove(&key)
            .is_some_and(|entry| entry.expanded);
        self.pending_order.retain(|pending| pending != &key);
        self.transcript.push(Entry::Tool(ToolEntry {
            card,
            expanded,
            image: None,
            started_at: None,
        }));
        self.finished_tool_index
            .insert(key, self.transcript.len() - 1);
    }

    fn reindex_finished_tools(&mut self) {
        let keys = {
            let mut pairs = self
                .finished_tool_index
                .iter()
                .map(|(key, index)| (*index, key.clone()))
                .collect::<Vec<_>>();
            pairs.sort_by_key(|(index, _)| *index);
            pairs.into_iter().map(|(_, key)| key).collect::<Vec<_>>()
        };
        self.finished_tool_index.clear();
        let mut next = 0usize;
        for (index, entry) in self.transcript.iter().enumerate() {
            if matches!(entry, Entry::Tool(_)) {
                if let Some(key) = keys.get(next) {
                    self.finished_tool_index.insert(key.clone(), index);
                }
                next += 1;
            }
        }
    }

    fn clear_press(&mut self) {
        self.press_cell = None;
        self.press_tool_key = None;
    }

    fn tool_key_for_target(&self, target: &ToggleTarget) -> Option<String> {
        match target {
            ToggleTarget::Pending(key) => Some(key.clone()),
            ToggleTarget::Transcript(index) => self
                .finished_tool_index
                .iter()
                .find_map(|(key, finished)| (*finished == *index).then(|| key.clone())),
        }
    }

    fn target_for_tool_key(&self, key: &str) -> Option<ToggleTarget> {
        if self.pending_tools.contains_key(key) {
            Some(ToggleTarget::Pending(key.to_string()))
        } else {
            self.finished_tool_index
                .get(key)
                .copied()
                .map(ToggleTarget::Transcript)
        }
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
                KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.toggle_latest_tool();
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
                let was_drag = self.scroll.drag().is_some();
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
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        self.press_cell = Some((mouse.column, mouse.row));
                        self.press_tool_key = if self.scroll.drag().is_some() {
                            None
                        } else {
                            self.tool_card_at_pointer(mouse.column, mouse.row)
                                .and_then(|(target, _)| self.tool_key_for_target(&target))
                        };
                    }
                    MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                        if self.press_cell != Some((mouse.column, mouse.row)) {
                            self.press_tool_key = None;
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        let same_cell = self.press_cell.take() == Some((mouse.column, mouse.row));
                        let press = self.press_tool_key.take();
                        // Layout can change between down and up. A stationary
                        // click follows the stable tool key, not the release
                        // hit-test or a positional transcript index.
                        if !was_drag && same_cell {
                            if let Some(target) =
                                press.and_then(|key| self.target_for_tool_key(&key))
                            {
                                self.toggle_tool_at(target);
                                return true;
                            }
                        }
                    }
                    _ => {}
                }
                true
            }
            Event::FocusGained => {
                self.clear_press();
                mouse_capture::reassert();
                false
            }
            Event::FocusLost => {
                self.clear_press();
                false
            }
            Event::Resize(_, _) => {
                self.clear_press();
                true
            }
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

    fn sync_history_geometry(&mut self, area: Rect, content_len: usize, width: usize) {
        self.history_area = area;
        self.history_width = width;
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
        self.sync_history_geometry(chunks[1], lines.len(), width);
        let start = self
            .scroll
            .visible_start(self.content_len, self.viewport_height);
        let end = start.saturating_add(self.viewport_height).min(lines.len());
        frame.render_widget(Paragraph::new(lines[start..end].to_vec()), chunks[1]);
        // Hover lift derives from the remembered pointer cell against this
        // frame's layout, so scroll, promotion, and toggles re-anchor it every
        // draw instead of caching stale content-line spans.
        if let Some(hovered) = self
            .last_mouse_position
            .and_then(|(column, row)| self.tool_card_at_pointer(column, row))
            .map(|(_, span)| span)
        {
            tool_card_hover::lift_lines(frame.buffer_mut(), chunks[1], start, hovered);
        }

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
                truncate_one_line("read-only · scroll · ctrl+o expand · q detach", width),
                Theme::dim(),
            ),
        ];
        frame.render_widget(Paragraph::new(footer).style(Style::default()), chunks[2]);
    }

    fn history_lines(&self, width: usize, status: Option<&RunStatus>) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let max_tool_output_lines = self.display.max_tool_output_lines();
        for item in self.history_items(status) {
            lines.extend(item.paint_lines(width, max_tool_output_lines));
        }
        if lines.is_empty() {
            lines.push(Line::styled("waiting for agent output...", Theme::dim()));
        }
        lines
    }

    fn history_items(&self, status: Option<&RunStatus>) -> Vec<HistoryItem<'_>> {
        let mut items = self
            .transcript
            .iter()
            .enumerate()
            .filter(|(_, entry)| !self.display.hides_entry(entry))
            .map(|(index, entry)| HistoryItem::Transcript { index, entry })
            .collect::<Vec<_>>();
        if self.display.shows_work_chrome() {
            items.extend(self.pending_order.iter().filter_map(|key| {
                self.pending_tools
                    .get(key)
                    .map(|tool| HistoryItem::Pending { key, tool })
            }));
        }
        let has_assistant = self
            .transcript
            .iter()
            .any(|entry| matches!(entry, Entry::Assistant(_)));
        items.extend(status_fallback_items(status, has_assistant));
        items
    }

    fn toggle_width(&self) -> usize {
        if self.history_width == 0 {
            TOGGLE_WIDTH_FALLBACK
        } else {
            self.history_width
        }
    }

    /// Content line under the pointer, excluding the scrollbar column.
    fn pointer_history_line(&self, column: u16, row: u16) -> Option<usize> {
        if !self.history_area.contains((column, row).into()) {
            return None;
        }
        if self
            .history_scrollbar()
            .is_some_and(|scrollbar| scrollbar.contains(column, row))
        {
            return None;
        }
        Some(
            self.scroll
                .visible_start(self.content_len, self.viewport_height)
                .saturating_add(usize::from(row.saturating_sub(self.history_area.y))),
        )
    }

    /// Toggleable card under the pointer: click target and hover-lift span.
    fn tool_card_at_pointer(&self, column: u16, row: u16) -> Option<(ToggleTarget, Range<usize>)> {
        let line = self.pointer_history_line(column, row)?;
        tool_card_at_line(
            self.history_items(self.status.as_ref()),
            line,
            self.toggle_width(),
            self.display.max_tool_output_lines(),
        )
    }

    fn toggle_latest_tool(&mut self) {
        let Some(target) = latest_toggle_target(
            self.history_items(self.status.as_ref()),
            self.toggle_width(),
            self.display.max_tool_output_lines(),
        ) else {
            return;
        };
        self.toggle_tool_at(target);
    }

    fn toggle_tool_at(&mut self, target: ToggleTarget) {
        let expand = !self.is_expanded(&target);
        for (index, entry) in self.transcript.iter_mut().enumerate() {
            if let Entry::Tool(tool) = entry {
                tool.expanded =
                    expand && matches!(&target, ToggleTarget::Transcript(i) if *i == index);
            }
        }
        for (key, tool) in &mut self.pending_tools {
            tool.expanded =
                expand && matches!(&target, ToggleTarget::Pending(pending) if pending == key);
        }
    }

    fn is_expanded(&self, target: &ToggleTarget) -> bool {
        match target {
            ToggleTarget::Transcript(index) => matches!(
                self.transcript.get(*index),
                Some(Entry::Tool(tool)) if tool.expanded
            ),
            ToggleTarget::Pending(key) => self
                .pending_tools
                .get(key)
                .is_some_and(|tool| tool.expanded),
        }
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
    if let Some(model) =
        crate::model_identity::PromptModel::from_run_status(status).map(|model| model.describe())
    {
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
