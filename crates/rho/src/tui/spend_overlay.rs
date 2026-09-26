//! `/spend` overlay: machine-wide AI spend from the usage ledger.
//!
//! A blocking task reads the ledger once and builds a report for every
//! [`SpendRange`]; `Tab` / `Shift+Tab` then switch ranges without another read.
//! [`SpendCache`] keeps the last reports for the session, so reopening paints
//! them at once while a fresh read runs, and only the first open shows a
//! spinner. `c` copies the visible report as plain text. Aggregation lives in
//! `crate::usage::report`; line layout lives in `spend_view`.

use std::{sync::Arc, time::Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
};

use super::{
    activity::LoadingSpinner,
    overlay_panel::{
        classify_panel_key, is_copy_key, overlay_panel_body_width, overlay_panel_layout,
        render_overlay_panel, terminal_area, OverlayPanelFrame, PanelKey, PanelScroll,
        PanelScrollTarget,
    },
    panel_pointer::PanelPointer,
    panel_text::indented_wrapped_lines,
    spend_view::{range_tabs_line, spend_body_lines},
    theme::Theme,
    App, ComposerMode, PanelOverlay,
};
use crate::usage::report::{catalog_route_pricing, load_spend_reports, SpendRange, SpendReports};

const TITLE: &str = "Spend";
const FOOTER: &str = "Tab range  c copy  Enter/Esc close";

type LoadResult = Result<Arc<SpendReports>, String>;

/// App-owned `/spend` data: the last finished reports and the one ledger read
/// in flight. Outlives the overlay so reopening is instant, and a read keeps
/// running when the overlay closes so rapid reopening never stacks reads.
#[derive(Debug, Default)]
pub(super) struct SpendCache {
    reports: Option<Arc<SpendReports>>,
    pending: Option<tokio::task::JoinHandle<LoadResult>>,
}

impl SpendCache {
    pub(super) fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn load_finished(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
    }

    /// Start a ledger read unless one is already running.
    fn refresh(&mut self) {
        // Unit tests inject `pending` instead of reading the real ledger.
        if self.pending.is_none() && !cfg!(test) {
            self.pending = Some(tokio::task::spawn_blocking(read_default_ledger));
        }
    }

    /// Drop the in-flight read at shutdown. A running `spawn_blocking` read
    /// cannot be cancelled; dropping the handle lets it finish unobserved.
    pub(super) fn abort(&mut self) {
        if let Some(handle) = self.pending.take() {
            handle.abort();
        }
    }
}

/// How the newest ledger read went, next to whatever reports are showing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LoadStatus {
    Running { started: Instant },
    Done,
    Failed(String),
}

#[derive(Clone, Debug)]
pub(super) struct SpendOverlay {
    /// Last finished reports; `None` until the first read of the session lands.
    reports: Option<Arc<SpendReports>>,
    load: LoadStatus,
    range: SpendRange,
    scroll: PanelScroll,
    /// Selection, scrollbar drag, and hover for this panel.
    pub(super) pointer: PanelPointer,
}

impl SpendOverlay {
    /// A spinner is on screen: nothing to show yet and a read is running.
    pub(super) fn shows_spinner(&self) -> bool {
        self.reports.is_none() && matches!(self.load, LoadStatus::Running { .. })
    }

    fn body_lines(&self, width: usize, now: Instant) -> Vec<Line<'static>> {
        let mut tabs = range_tabs_line(self.range);
        let mut notice = Vec::new();
        match (&self.reports, &self.load) {
            (Some(_), LoadStatus::Running { .. }) => tabs
                .spans
                .push(Span::styled("   updating", Theme::dim_italic())),
            (Some(_), LoadStatus::Failed(error)) => {
                tabs.spans
                    .push(Span::styled("   not updated", Theme::warning()));
                notice = indented_wrapped_lines(
                    &format!("could not refresh usage ledger: {error}"),
                    2,
                    width,
                    Theme::dim(),
                );
            }
            (Some(_), LoadStatus::Done) | (None, _) => {}
        }
        let mut lines = vec![tabs];
        lines.extend(notice);
        lines.push(Line::default());
        match (&self.reports, &self.load) {
            (Some(reports), _) => {
                lines.extend(spend_body_lines(self.range, reports.get(self.range), width));
            }
            (None, LoadStatus::Running { started }) => {
                let spinner = LoadingSpinner::frame_since(*started, now);
                lines.push(Line::styled(
                    format!("  {spinner} reading usage ledger"),
                    Theme::dim(),
                ));
            }
            (None, LoadStatus::Failed(error)) => lines.extend(indented_wrapped_lines(
                &format!("could not read usage ledger: {error}"),
                2,
                width,
                Theme::error(),
            )),
            // Unreachable: a finished read always leaves reports or an error.
            (None, LoadStatus::Done) => {}
        }
        lines
    }

    /// The visible report as plain text, without refresh status or errors.
    fn report_text(&self, width: usize) -> Option<String> {
        let reports = self.reports.as_ref()?;
        let mut lines = vec![range_tabs_line(self.range), Line::default()];
        lines.extend(spend_body_lines(self.range, reports.get(self.range), width));
        Some(
            lines
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    fn cycle_range(&mut self, step: isize) {
        self.range = self.range.cycled(step);
        self.scroll = PanelScroll::default();
        self.pointer.clear_selection();
    }
}

/// Read the default ledger and build every range. Runs on a blocking thread.
fn read_default_ledger() -> LoadResult {
    let path = crate::paths::usage_database_path().map_err(|error| error.to_string())?;
    let now = chrono::Local::now().naive_local();
    load_spend_reports(&path, &chrono::Local, now, catalog_route_pricing)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

impl App {
    pub(super) fn execute_spend_command(&mut self) -> anyhow::Result<()> {
        self.spend.refresh();
        let load = if self.spend.is_loading() {
            LoadStatus::Running {
                started: Instant::now(),
            }
        } else {
            LoadStatus::Done
        };
        self.input_ui
            .set_composer(ComposerMode::Panel(PanelOverlay::Spend(Box::new(
                SpendOverlay {
                    reports: self.spend.reports.clone(),
                    load,
                    range: SpendRange::Today,
                    scroll: PanelScroll::default(),
                    pointer: PanelPointer::default(),
                },
            ))));
        self.set_status_quiet("spend");
        Ok(())
    }

    fn spend_overlay(&self) -> Option<&SpendOverlay> {
        match self.input_ui.composer() {
            ComposerMode::Panel(PanelOverlay::Spend(overlay)) => Some(overlay),
            _ => None,
        }
    }

    fn spend_overlay_mut(&mut self) -> Option<&mut SpendOverlay> {
        match self.input_ui.composer_mut() {
            ComposerMode::Panel(PanelOverlay::Spend(overlay)) => Some(overlay),
            _ => None,
        }
    }

    pub(super) fn close_spend_overlay(&mut self) {
        if self.spend_overlay().is_some() {
            self.input_ui.set_composer(ComposerMode::Input);
        }
    }

    /// Store a finished read in the cache and, if the overlay is open, show
    /// it. Returns whether anything changed on screen.
    pub(super) async fn poll_spend_load(&mut self) -> bool {
        if !self.spend.load_finished() {
            return false;
        }
        let Some(handle) = self.spend.pending.take() else {
            return false;
        };
        let loaded = match handle.await {
            Ok(result) => result,
            Err(error) => Err(error.to_string()),
        };
        if let Ok(reports) = &loaded {
            self.spend.reports = Some(Arc::clone(reports));
        }
        let Some(overlay) = self.spend_overlay_mut() else {
            return false;
        };
        // A failed refresh keeps the reports on screen; the status says so.
        overlay.load = match loaded {
            Ok(reports) => {
                overlay.reports = Some(reports);
                LoadStatus::Done
            }
            Err(error) => LoadStatus::Failed(error),
        };
        overlay.pointer.clear_selection();
        true
    }

    pub(super) fn spend_overlay_frame(
        &self,
        area: Rect,
        now: Instant,
    ) -> Option<OverlayPanelFrame> {
        let overlay = self.spend_overlay()?;
        let lines = overlay.body_lines(overlay_panel_body_width(area), now);
        Some(render_overlay_panel(
            TITLE,
            FOOTER,
            lines,
            overlay.scroll.offset(),
            area,
        ))
    }

    pub(super) fn handle_spend_overlay_key(
        &mut self,
        key: KeyEvent,
        terminal: &ratatui::DefaultTerminal,
    ) -> bool {
        if self.spend_overlay().is_none() {
            return false;
        }
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Tab | KeyCode::Right) => {
                self.cycle_spend_range(1);
                return true;
            }
            (_, KeyCode::BackTab) | (KeyModifiers::NONE, KeyCode::Left) => {
                self.cycle_spend_range(-1);
                return true;
            }
            _ if is_copy_key(key) => {
                self.copy_spend_report(Instant::now());
                return true;
            }
            _ => {}
        }
        match classify_panel_key(key) {
            PanelKey::Close => {
                self.close_spend_overlay();
                true
            }
            PanelKey::Scroll(target) => {
                if let Some(area) = terminal_area(terminal) {
                    self.scroll_spend_overlay(area, target);
                }
                true
            }
            PanelKey::Passthrough => false,
            PanelKey::Swallow => true,
        }
    }

    fn cycle_spend_range(&mut self, step: isize) {
        if let Some(overlay) = self.spend_overlay_mut() {
            overlay.cycle_range(step);
        }
    }

    /// Copy the visible range as plain text at the body's width cap, so
    /// columns line up when pasted. Does nothing until a report exists.
    fn copy_spend_report(&mut self, now: Instant) {
        if let Some(text) = self
            .spend_overlay()
            .and_then(|overlay| overlay.report_text(usize::MAX))
        {
            self.copy_text(&text, now);
        }
    }

    pub(super) fn scroll_spend_overlay(&mut self, area: Rect, target: PanelScrollTarget) -> bool {
        let Some(overlay) = self.spend_overlay() else {
            return false;
        };
        let body_len = overlay
            .body_lines(overlay_panel_body_width(area), Instant::now())
            .len();
        let body_rows = overlay_panel_layout(area, body_len).body_rows;
        if let Some(overlay) = self.spend_overlay_mut() {
            overlay.scroll.apply(target, body_len, body_rows);
        }
        true
    }

    pub(super) fn clamp_spend_overlay_scroll(&mut self, terminal: &ratatui::DefaultTerminal) {
        let Some(offset) = self.spend_overlay().map(|overlay| overlay.scroll.offset()) else {
            return;
        };
        if let Some(area) = terminal_area(terminal) {
            self.scroll_spend_overlay(area, PanelScrollTarget::Absolute(offset));
        }
    }
}

#[cfg(test)]
#[path = "spend_overlay_tests.rs"]
mod tests;
