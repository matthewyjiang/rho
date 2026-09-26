//! `/doctor` dashboard overlay: one status marker per check, section
//! spinners while probes run, and a hint under each issue.
//!
//! Check policy lives in `crate::doctor`. This module owns the overlay
//! state, the pending probe tasks, and how rows are laid out. Probes are
//! spawned, never awaited inline, so the event loop and stream draining stay
//! responsive while a child process or endpoint is slow.

use std::time::Instant;

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{
    activity::LoadingSpinner,
    background_tasks::{TaskId, UiOutput},
    overlay_panel::{PanelBody, PanelState},
    panel_text::{heading_with_status, indented_wrapped_lines, truncate_to},
    render::display_width,
    theme::Theme,
    App, ComposerMode, PanelOverlay,
};
use crate::doctor::{
    build_report, plan_probes, probe_checks, run_probe, DoctorCheck, DoctorInputs, DoctorProbeGate,
    DoctorProbeId, DoctorProbeOutcome, DoctorReport, DoctorSection, DoctorStatus, HerdrProbe,
};

const TITLE: &str = "Doctor";
const FOOTER: &str = "Enter/Esc close";
const HINT_INDENT: usize = 4;
/// Columns kept free of the label column: marker gutter plus a minimum
/// summary so a long label never pushes the status off screen.
const ROW_CHROME_WIDTH: usize = 12;
const FALLBACK_SPINNER: &str = "⠙";

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DoctorOverlay {
    report: DoctorReport,
    panel: PanelState,
    /// Spinner phase anchor.
    checking_started: Instant,
}

impl DoctorOverlay {
    pub(super) fn is_checking(&self) -> bool {
        self.report.is_checking()
    }
}

impl PanelBody for DoctorOverlay {
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
        let spinner = self
            .is_checking()
            .then(|| LoadingSpinner::frame_since(self.checking_started, now));
        overlay_body_lines(self, width, spinner)
    }

    /// Key handling is synchronous, so probes are aborted without being
    /// awaited. Probe children are `kill_on_drop`.
    fn close(self: Box<Self>, app: &mut App) {
        app.abort_doctor_probes();
    }
}

/// Live probes stay out of unit tests, mirroring `/limits`.
fn probe_gate() -> DoctorProbeGate {
    if cfg!(test) {
        DoctorProbeGate::Disabled
    } else {
        DoctorProbeGate::Live
    }
}

impl App {
    pub(super) fn execute_doctor_command(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.start_doctor_command()?;
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }

    /// Open the overlay with instant rows and spawn one task per probe. Safe
    /// during a model turn: nothing here awaits a child or the network.
    pub(super) fn start_doctor_command(&mut self) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        self.refresh_available_auths();
        let config_path = self.info.services.config_repository.configured_path()?;
        let session_root = crate::paths::rho_dir()?.join("sessions");
        let clipboard = crate::clipboard::doctor_report();
        self.abort_doctor_probes();
        let probes = plan_probes(&config, &self.info.runtime.provider, probe_gate());
        let report = build_report(DoctorInputs {
            provider: &self.info.runtime.provider,
            model: &self.info.runtime.model,
            auth: &self.info.runtime.auth,
            available_auths: &self.available_auths,
            credential_store: self.credential_store.as_ref(),
            config_path: &config_path,
            session_root: &session_root,
            herdr: HerdrProbe::from_reporter(&self.info.services.herdr),
            clipboard: &clipboard,
            mcp_report: &self.mcp_report,
            plugins_report: &self.plugins_report,
            probes: &probes,
        });
        for id in probes {
            self.spawn_doctor_probe(id.clone(), run_probe(id, self.credential_store.clone()));
        }
        self.input_ui
            .set_composer(ComposerMode::Panel(PanelOverlay::Doctor(DoctorOverlay {
                report,
                panel: PanelState::default(),
                checking_started: Instant::now(),
            })));
        self.set_status("doctor");
        Ok(())
    }

    pub(super) fn doctor_overlay_open(&self) -> bool {
        matches!(
            self.input_ui.composer(),
            ComposerMode::Panel(PanelOverlay::Doctor(_))
        )
    }

    /// Spawn one probe. A join error reads as a failed probe.
    pub(super) fn spawn_doctor_probe(
        &mut self,
        id: DoctorProbeId,
        probe: impl std::future::Future<Output = DoctorProbeOutcome> + Send + 'static,
    ) {
        self.tasks
            .spawn(TaskId::DoctorProbe(id.clone()), probe, move |result| {
                UiOutput::DoctorProbe(result.unwrap_or(DoctorProbeOutcome::Failed(id))).into()
            });
    }

    fn abort_doctor_probes(&mut self) {
        self.tasks.abort(|id| matches!(id, TaskId::DoctorProbe(_)));
    }

    pub(super) async fn cancel_doctor_command(&mut self) {
        self.tasks
            .cancel(|id| matches!(id, TaskId::DoctorProbe(_)))
            .await;
    }

    /// Approvals and other set_composer replacements do not go through panel
    /// close; drop leftover children once the overlay is gone.
    pub(super) async fn cancel_orphaned_doctor_probes(&mut self) {
        if !self.doctor_overlay_open() {
            self.cancel_doctor_command().await;
        }
    }

    pub(super) fn apply_doctor_probe(&mut self, outcome: &DoctorProbeOutcome) -> bool {
        let active_provider = self.info.runtime.provider.clone();
        let Some(overlay) = self.doctor_overlay_mut() else {
            return false;
        };
        overlay
            .report
            .replace_checks(probe_checks(outcome, &active_provider));
        // Rows may have moved under a selection anchored by line.
        overlay.panel.pointer.clear_selection();
        true
    }

    fn doctor_overlay_mut(&mut self) -> Option<&mut DoctorOverlay> {
        match self.input_ui.composer_mut() {
            ComposerMode::Panel(PanelOverlay::Doctor(overlay)) => Some(overlay),
            _ => None,
        }
    }
}

/// Pure layout of the whole panel body. `spinner` is the current frame while
/// any probe is pending, `None` once the report is settled.
fn overlay_body_lines(
    overlay: &DoctorOverlay,
    width: usize,
    spinner: Option<&'static str>,
) -> Vec<Line<'static>> {
    let report = &overlay.report;
    let label_width = report
        .checks()
        .map(|check| display_width(&check.label))
        .max()
        .unwrap_or(0)
        .min(width.saturating_sub(ROW_CHROME_WIDTH));

    let mut lines = vec![headline_line(report, width)];
    for section in &report.sections {
        lines.push(Line::default());
        lines.push(section_heading(section, width, spinner));
        for check in &section.checks {
            lines.extend(check_lines(check, label_width, width, spinner));
        }
    }
    lines
}

fn headline_line(report: &DoctorReport, width: usize) -> Line<'static> {
    let summary = report.summary();
    let style = if summary.fail > 0 || summary.warn > 0 {
        Theme::text_strong()
    } else {
        Theme::dim()
    };
    Line::from(Span::styled(truncate_to(&report.headline(), width), style))
}

fn section_heading(
    section: &DoctorSection,
    width: usize,
    spinner: Option<&'static str>,
) -> Line<'static> {
    let status = if section.is_checking() {
        format!("{} checking", spinner.unwrap_or(FALLBACK_SPINNER))
    } else {
        String::new()
    };
    heading_with_status(section.id.label(), &status, width)
}

fn check_lines(
    check: &DoctorCheck,
    label_width: usize,
    width: usize,
    spinner: Option<&'static str>,
) -> Vec<Line<'static>> {
    let (glyph, marker_style) = marker(check.status, spinner);
    let label = truncate_to(&check.label, label_width.max(1));
    let label_pad = " ".repeat(label_width.saturating_sub(display_width(&label)));
    let used = 2 + display_width(glyph) + 1 + label_width + 2;
    let summary = truncate_to(&check.summary, width.saturating_sub(used).max(1));
    let summary_style = match check.status {
        DoctorStatus::Ok | DoctorStatus::Info => Theme::text(),
        DoctorStatus::Warn | DoctorStatus::Fail => marker_style,
        DoctorStatus::Checking => Theme::dim(),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("  ", Theme::text()),
        Span::styled(glyph.to_string(), marker_style),
        Span::styled(" ", Theme::text()),
        Span::styled(format!("{label}{label_pad}"), Theme::text()),
        Span::styled("  ", Theme::text()),
        Span::styled(summary, summary_style),
    ])];
    if let Some(hint) = check.hint.as_deref().filter(|_| check.status.is_issue()) {
        lines.extend(indented_wrapped_lines(
            hint,
            HINT_INDENT,
            width,
            Theme::dim(),
        ));
    }
    lines
}

fn marker(status: DoctorStatus, spinner: Option<&'static str>) -> (&'static str, Style) {
    match status {
        DoctorStatus::Ok => ("✓", Theme::success()),
        DoctorStatus::Info => ("·", Theme::dim()),
        DoctorStatus::Warn => ("!", Theme::warning()),
        DoctorStatus::Fail => ("✗", Theme::error()),
        DoctorStatus::Checking => (spinner.unwrap_or(FALLBACK_SPINNER), Theme::dim()),
    }
}

#[cfg(test)]
#[path = "doctor_overlay_tests.rs"]
mod tests;
