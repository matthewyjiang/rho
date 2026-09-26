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
use ratatui::text::{Line, Span};

use super::{
    activity::LoadingSpinner,
    background_tasks::{TaskId, TaskOutput, UiOutput},
    overlay_panel::{PanelBody, PanelKeyOutcome, PanelScroll, PanelState},
    panel_text::indented_wrapped_lines,
    spend_view::{range_tabs_line, spend_body_lines},
    theme::Theme,
    App, ComposerMode, PanelOverlay,
};
use crate::usage::report::{catalog_route_pricing, load_spend_reports, SpendRange, SpendReports};

const TITLE: &str = "Spend";
const FOOTER: &str = "Tab range  c copy  Enter/Esc close";

pub(super) type LoadResult = Result<Arc<SpendReports>, String>;

/// App-owned `/spend` data: the last finished reports. Outlives the overlay
/// so reopening is instant. The one ledger read in flight is a
/// `TaskId::SpendLoad` background task; it keeps running when the overlay
/// closes so rapid reopening never stacks reads.
#[derive(Debug, Default)]
pub(super) struct SpendCache {
    reports: Option<Arc<SpendReports>>,
}

/// Background-task wrapper for a ledger read; a join error reads as a failure.
fn spend_load_output(result: Result<LoadResult, tokio::task::JoinError>) -> TaskOutput {
    UiOutput::SpendLoad(result.unwrap_or_else(|error| Err(error.to_string()))).into()
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
    panel: PanelState,
}

impl SpendOverlay {
    /// A spinner is on screen: nothing to show yet and a read is running.
    pub(super) fn shows_spinner(&self) -> bool {
        self.reports.is_none() && matches!(self.load, LoadStatus::Running { .. })
    }

    fn spend_lines(&self, width: usize, now: Instant) -> Vec<Line<'static>> {
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
        self.panel.scroll = PanelScroll::default();
        self.panel.pointer.clear_selection();
    }
}

impl PanelBody for SpendOverlay {
    fn state(&self) -> &PanelState {
        &self.panel
    }

    fn state_mut(&mut self) -> &mut PanelState {
        &mut self.panel
    }

    fn title(&self) -> &str {
        TITLE
    }

    fn footer(&self) -> &str {
        FOOTER
    }

    fn body_lines(&self, width: usize, now: Instant) -> Vec<Line<'static>> {
        self.spend_lines(width, now)
    }

    /// The visible range at the body's width cap, so columns line up when
    /// pasted. Nothing until a report exists.
    fn copy_text(&self) -> Option<String> {
        self.report_text(usize::MAX)
    }

    fn handle_key(&mut self, key: KeyEvent) -> PanelKeyOutcome {
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Tab | KeyCode::Right) => self.cycle_range(1),
            (_, KeyCode::BackTab) | (KeyModifiers::NONE, KeyCode::Left) => self.cycle_range(-1),
            _ => return PanelKeyOutcome::Unhandled,
        }
        PanelKeyOutcome::Handled
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
    fn spend_loading(&self) -> bool {
        self.tasks.contains(|id| *id == TaskId::SpendLoad)
    }

    /// Start a ledger read unless one is already running.
    fn refresh_spend(&mut self) {
        // Unit tests inject a load task instead of reading the real ledger.
        if !self.spend_loading() && !cfg!(test) {
            self.tasks
                .spawn_blocking(TaskId::SpendLoad, read_default_ledger, spend_load_output);
        }
    }

    pub(super) fn execute_spend_command(&mut self) -> anyhow::Result<()> {
        self.refresh_spend();
        let load = if self.spend_loading() {
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
                    panel: PanelState::default(),
                },
            ))));
        self.set_status_quiet("spend");
        Ok(())
    }

    fn spend_overlay_mut(&mut self) -> Option<&mut SpendOverlay> {
        match self.input_ui.composer_mut() {
            ComposerMode::Panel(PanelOverlay::Spend(overlay)) => Some(overlay),
            _ => None,
        }
    }

    /// Store a finished read in the cache and, if the overlay is open, show
    /// it. Returns whether anything changed on screen.
    pub(super) fn apply_spend_load(&mut self, loaded: LoadResult) -> bool {
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
        overlay.panel.pointer.clear_selection();
        true
    }
}

#[cfg(test)]
#[path = "spend_overlay_tests.rs"]
mod tests;
