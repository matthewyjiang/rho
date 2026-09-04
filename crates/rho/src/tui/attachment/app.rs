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
    text::Line,
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use rho_sdk::model::ContextUsage;
use rho_tools::tool_card::ToolCard;

use crate::{
    herdr::{HerdrReporter, HerdrState},
    run_artifacts::{AttachmentEvent, AttachmentReader},
    subagent::{self, RunState, RunStatus},
};

use super::super::{
    live_started_at,
    model_performance::ModelPerformanceAggregate,
    mouse_capture,
    provider_attempt::ProviderAttempt,
    render::truncate_one_line,
    scrollbar::{HistoryScrollChrome, HistoryScrollbar, ScrollbarMouseInput},
    terminal_events::TerminalEvents,
    theme::Theme,
    tool_card_hover,
    usage_cost::AttemptAwareRunUsage,
    Entry, HistoryScroll, ReasoningChrome, ReasoningEntry, ToolEntry, HISTORY_MOUSE_SCROLL_LINES,
    HISTORY_SCROLLBAR_REVEAL_DURATION,
};
#[cfg(test)]
use super::chrome::format_run_cost;
use super::chrome::{
    activity_metrics_line, footer_line, header_title_line, herdr_status, identity_line,
    AttachChrome,
};
use super::tool_toggle::{
    latest_toggle_target, status_fallback_items, tool_card_at_line, HistoryItem, PaintedHistory,
    ToggleTarget,
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const TOGGLE_WIDTH_FALLBACK: usize = 80;

/// Result of routing one terminal event through the attach view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachInput {
    Ignored,
    Handled,
    Leave,
    Quit,
}

impl AttachInput {
    pub(crate) fn redraws(self) -> bool {
        !matches!(self, Self::Ignored)
    }
}

/// Display policy for `rho attach`, mirrored from interactive config.
///
/// Keep ingest complete (including hidden reasoning) so journal replay and
/// provider-attempt bookkeeping stay stable; filter at render time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttachmentDisplaySettings {
    pub show_reasoning_output: bool,
    pub zen_mode: bool,
    pub max_tool_output_lines: usize,
}

impl Default for AttachmentDisplaySettings {
    fn default() -> Self {
        Self::from_config(&crate::config::Config::default())
    }
}

impl AttachmentDisplaySettings {
    pub(crate) fn from_runtime(
        show_reasoning_output: bool,
        zen_mode: bool,
        max_tool_output_lines: usize,
    ) -> Self {
        Self {
            show_reasoning_output,
            zen_mode,
            max_tool_output_lines: max_tool_output_lines.max(1),
        }
    }

    pub(crate) fn from_config(config: &crate::config::Config) -> Self {
        Self::from_runtime(
            config.show_reasoning_output,
            config.zen_mode,
            config.max_tool_output_lines,
        )
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
        self.max_tool_output_lines
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
    theme: &str,
    herdr: HerdrReporter,
) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("rho attach requires an interactive terminal");
    }
    let _syntax_warmup = tokio::task::spawn_blocking(crate::tui::syntax::warm_syntax_set);
    let mut terminal = ratatui::init();
    let _restore_terminal = RestoreTerminal {
        mouse_capture: mouse_capture::Guard::acquire(),
    };
    Theme::initialize_from_terminal();
    Theme::apply_committed(theme);
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
    let result = AttachmentApp::new(&id, directory, display)
        .run(&mut terminal, &herdr)
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

pub(crate) struct AttachmentApp {
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
    /// One average for the attached run. A subagent uses one model; a mid-run
    /// tier fallback blends into that same average instead of keyed profiles.
    model_performance: ModelPerformanceAggregate,
    provider_attempt: ProviderAttempt,
    status: Option<RunStatus>,
    /// Last whole-second live elapsed painted into the header (running runs only).
    last_drawn_elapsed_secs: Option<u64>,
    scroll: HistoryScrollChrome,
    last_mouse_position: Option<(u16, u16)>,
    /// Stable tool key under the last left-button press, if any. Survives
    /// pending→transcript promotion and provider resets that shift indexes.
    press_tool_key: Option<String>,
    press_cell: Option<(u16, u16)>,
    /// Current transcript index for each finished tool key.
    finished_tool_index: BTreeMap<String, usize>,
    /// Cached history render shared by draw and hit-testing. Invalidated on
    /// content, status, toggle, and width changes so mouse and scroll events
    /// reuse it instead of re-rendering the whole transcript.
    painted: Option<PaintedHistory>,
    viewport_height: usize,
    history_area: Rect,
    history_width: usize,
    content_len: usize,
    syntax_ready: bool,
}

impl AttachmentApp {
    pub(crate) fn new(id: &str, directory: PathBuf, display: AttachmentDisplaySettings) -> Self {
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
            model_performance: ModelPerformanceAggregate::default(),
            provider_attempt: ProviderAttempt::default(),
            status: None,
            last_drawn_elapsed_secs: None,
            scroll: HistoryScrollChrome::default(),
            last_mouse_position: None,
            press_tool_key: None,
            press_cell: None,
            finished_tool_index: BTreeMap::new(),
            painted: None,
            viewport_height: 0,
            history_area: Rect::default(),
            history_width: 0,
            content_len: 0,
            syntax_ready: crate::tui::syntax::syntax_set_ready(),
        }
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.id
    }

    pub(crate) fn should_redraw(&self, now: Instant) -> bool {
        self.scroll.should_render(now) || self.live_elapsed_secs() != self.last_drawn_elapsed_secs
    }

    pub(crate) fn note_drawn(&mut self) {
        self.last_drawn_elapsed_secs = self.live_elapsed_secs();
    }

    async fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        herdr: &HerdrReporter,
    ) -> anyhow::Result<()> {
        let mut terminal_events = TerminalEvents::new();
        let mut refresh = tokio::time::interval(REFRESH_INTERVAL);
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut reported_state = None;
        self.refresh()?;
        self.report_herdr(herdr, &mut reported_state).await;
        terminal.draw(|frame| self.draw(frame, AttachChrome::Standalone))?;
        self.note_drawn();

        loop {
            let redraw = tokio::select! {
                event = terminal_events.next() => {
                    match self.handle_event(event?) {
                        AttachInput::Leave | AttachInput::Quit => break,
                        outcome => outcome.redraws(),
                    }
                }
                _ = refresh.tick() => {
                    let changed = self.refresh()?;
                    self.report_herdr(herdr, &mut reported_state).await;
                    let elapsed_advanced =
                        self.live_elapsed_secs() != self.last_drawn_elapsed_secs;
                    // Keep redrawing while the auto-hide scrollbar is visible, and
                    // when a live run's whole-second elapsed label advances without I/O.
                    changed
                        || self.scroll.should_render(Instant::now())
                        || elapsed_advanced
                },
            };
            if redraw {
                terminal.draw(|frame| self.draw(frame, AttachChrome::Standalone))?;
                self.note_drawn();
            }
        }
        Ok(())
    }

    async fn report_herdr(&self, herdr: &HerdrReporter, reported_state: &mut Option<RunState>) {
        let Some(status) = &self.status else {
            return;
        };
        if *reported_state == Some(status.state) {
            return;
        }
        let (state, message) = herdr_status(&self.id, status);
        herdr
            .report_state(state, Some(&message), Some(&self.id))
            .await;
        *reported_state = Some(status.state);
    }

    pub(crate) fn refresh(&mut self) -> anyhow::Result<bool> {
        let mut changed = self.take_syntax_ready_change();
        let events = self.reader.read_new()?;
        changed |= !events.is_empty();
        for event in events {
            self.apply_event(event);
        }
        let status_path = self.directory.join(subagent::RESULT_FILE_NAME);
        if let Some(status) = subagent::read_status(&status_path) {
            let status_changed = self.status.as_ref() != Some(&status);
            if status_changed {
                // Status feeds the fallback history items.
                self.invalidate_painted();
            }
            changed |= status_changed;
            self.status = Some(status);
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
        if !matches!(
            event,
            AttachmentEvent::ContextUsage(_)
                | AttachmentEvent::Usage(_)
                | AttachmentEvent::ModelCallCompleted { .. }
                | AttachmentEvent::StepStarted
        ) {
            self.invalidate_painted();
        }
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
            AttachmentEvent::ModelCallCompleted {
                generation_output_tokens,
                generation_time_ms,
            } => {
                self.model_performance.record_resolved(
                    generation_output_tokens,
                    Duration::from_millis(generation_time_ms),
                );
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
        self.pending_tools
            .insert(key, ToolEntry::new(card, expanded, None, started_at));
    }

    fn finish_pending_tool(&mut self, key: Option<String>, card: ToolCard) {
        let key = attachment_tool_key(key);
        let expanded = self
            .pending_tools
            .remove(&key)
            .is_some_and(|entry| entry.expanded);
        self.pending_order.retain(|pending| pending != &key);
        self.transcript
            .push(Entry::Tool(ToolEntry::new(card, expanded, None, None)));
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

    fn invalidate_painted(&mut self) {
        self.painted = None;
    }

    fn take_syntax_ready_change(&mut self) -> bool {
        let ready = crate::tui::syntax::syntax_set_ready();
        if ready == self.syntax_ready {
            return false;
        }
        self.syntax_ready = ready;
        self.invalidate_painted();
        true
    }

    /// Rebuild the cached history render when missing or wrapped for another
    /// width. Content changes drop the cache via [`Self::invalidate_painted`].
    fn ensure_painted(&mut self, width: usize) -> &PaintedHistory {
        if self
            .painted
            .as_ref()
            .is_none_or(|painted| painted.width != width)
        {
            self.painted = Some(self.paint_history(width));
        }
        self.painted.as_ref().expect("painted history just ensured")
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

    pub(crate) fn handle_event(&mut self, event: Event) -> AttachInput {
        let now = Instant::now();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    AttachInput::Quit
                }
                KeyCode::Char('q') | KeyCode::Esc => AttachInput::Leave,
                KeyCode::Up => {
                    self.scroll_lines(now, -1);
                    AttachInput::Handled
                }
                KeyCode::Down => {
                    self.scroll_lines(now, 1);
                    AttachInput::Handled
                }
                KeyCode::PageUp => {
                    self.scroll_lines(now, -(self.viewport_height.max(1) as isize));
                    AttachInput::Handled
                }
                KeyCode::PageDown => {
                    self.scroll_lines(now, self.viewport_height.max(1) as isize);
                    AttachInput::Handled
                }
                KeyCode::Home => {
                    self.scroll
                        .set_top_line(self.content_len, self.viewport_height, 0);
                    if !matches!(self.scroll.scroll(), HistoryScroll::Bottom) {
                        self.scroll.reveal(now, HISTORY_SCROLLBAR_REVEAL_DURATION);
                    }
                    AttachInput::Handled
                }
                KeyCode::End => {
                    self.scroll.scroll_to_bottom();
                    AttachInput::Handled
                }
                KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.toggle_latest_tool();
                    AttachInput::Handled
                }
                _ => AttachInput::Ignored,
            },
            Event::Mouse(mouse) => {
                if matches!(mouse.kind, MouseEventKind::Moved)
                    && self.last_mouse_position == Some((mouse.column, mouse.row))
                {
                    return AttachInput::Ignored;
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
                                return AttachInput::Handled;
                            }
                        }
                    }
                    _ => {}
                }
                AttachInput::Handled
            }
            Event::FocusGained => {
                self.clear_press();
                mouse_capture::reassert();
                AttachInput::Ignored
            }
            Event::FocusLost => {
                self.clear_press();
                AttachInput::Ignored
            }
            Event::Resize(_, _) => {
                self.clear_press();
                AttachInput::Handled
            }
            _ => AttachInput::Ignored,
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

    pub(crate) fn draw(&mut self, frame: &mut Frame<'_>, chrome: AttachChrome) {
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
        let rate = self.model_performance.summary().rounded_generation_rate();
        let activity_metrics = activity_metrics_line(
            activity,
            self.context_usage.as_ref(),
            self.run_usage.current(),
            status,
            rate,
        );
        let header = vec![
            header_title_line(&self.id, agent_id, state, status),
            Line::styled(truncate_one_line(&identity, width), Theme::dim()),
            Line::styled(truncate_one_line(&activity_metrics, width), Theme::dim()),
            Line::styled("─".repeat(width.max(1)), Theme::dim()),
        ];
        frame.render_widget(Paragraph::new(header), chunks[0]);

        // Pending cards paint live elapsed labels. Invalidate here so a
        // key-driven redraw on a second boundary still refreshes them.
        if self.live_elapsed_secs() != self.last_drawn_elapsed_secs
            && !self.pending_tools.is_empty()
        {
            self.invalidate_painted();
        }
        let content_len = self.ensure_painted(width).lines.len();
        self.sync_history_geometry(chunks[1], content_len, width);
        let start = self
            .scroll
            .visible_start(self.content_len, self.viewport_height);
        let end = start.saturating_add(self.viewport_height).min(content_len);
        let visible = self.ensure_painted(width).lines[start..end].to_vec();
        frame.render_widget(Paragraph::new(visible), chunks[1]);
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
            footer_line(chrome, width),
        ];
        frame.render_widget(Paragraph::new(footer).style(Style::default()), chunks[2]);
    }

    /// Paint the full history for `width`, including the placeholder shown
    /// before any agent output arrives.
    fn paint_history(&self, width: usize) -> PaintedHistory {
        let mut painted = PaintedHistory::paint(
            self.history_items(self.status.as_ref()),
            width,
            self.display.max_tool_output_lines(),
        );
        if painted.lines.is_empty() {
            painted
                .lines
                .push(Line::styled("waiting for agent output...", Theme::dim()));
        }
        painted
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
    fn tool_card_at_pointer(
        &mut self,
        column: u16,
        row: u16,
    ) -> Option<(ToggleTarget, Range<usize>)> {
        let line = self.pointer_history_line(column, row)?;
        let width = self.toggle_width();
        tool_card_at_line(&self.ensure_painted(width).cards, line)
    }

    fn toggle_latest_tool(&mut self) {
        let width = self.toggle_width();
        let Some(target) = latest_toggle_target(&self.ensure_painted(width).cards) else {
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
        self.invalidate_painted();
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
        (StreamTarget::Assistant, _) => transcript.push(Entry::Assistant(text.into())),
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

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
