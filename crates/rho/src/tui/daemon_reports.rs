//! `/context` and `/hooks` against a daemon session: the daemon reads the
//! report from the session it owns, and the overlay opens when it answers.
//!
//! An answer opens its overlay only while the person still waits on it:
//! [`App::awaited_daemon_report`] names the report asked for last, and a
//! prompt or a switch to another session stops the wait, so an answer that
//! arrives after the person moved on is dropped.

use rho_protocol::SessionReport;

use super::{App, ComposerMode, Entry};

/// What the person asked to see, for the status line and a refusal.
fn name(report: SessionReport) -> &'static str {
    match report {
        SessionReport::Context => "context",
        SessionReport::Hooks => "hooks",
        // `SessionReport` is non-exhaustive; the TUI asks only for the above.
        _ => "report",
    }
}

impl App {
    /// Asks the daemon for `report` on the followed session.
    pub(super) fn request_daemon_report(&mut self, report: SessionReport) {
        let session = self
            .info
            .session
            .daemon
            .as_ref()
            .expect("a daemon report requires a session");
        match session.inspect(report) {
            Ok(()) => {
                self.awaited_daemon_report = Some(report);
                self.set_status_quiet(format!("reading {}", name(report)));
            }
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not read {}: {error}",
                name(report)
            ))),
        }
    }

    /// Opens the overlay for a report the daemon answered with.
    pub(super) fn show_daemon_report(
        &mut self,
        report: SessionReport,
        result: Result<serde_json::Value, String>,
    ) {
        // An answer to a report the person asked for before another, or
        // before moving on, is not what they wait on.
        if self.awaited_daemon_report != Some(report) {
            return;
        }
        self.awaited_daemon_report = None;
        // A late answer must not replace what the person opened meanwhile.
        if result.is_ok() && !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return;
        }
        let shown = result.and_then(|value| match report {
            SessionReport::Context => serde_json::from_value(value)
                .map(|report| self.show_context_report(report))
                .map_err(|error| error.to_string()),
            SessionReport::Hooks => serde_json::from_value(value)
                .map(|report| self.show_hooks_report(report))
                .map_err(|error| error.to_string()),
            _ => Err("this rho does not know the report".to_owned()),
        });
        if let Err(error) = shown {
            self.insert_entry(&Entry::Error(format!(
                "could not read {}: {error}",
                name(report)
            )));
            self.set_status_quiet(format!("could not read {}", name(report)));
        }
    }
}

#[cfg(test)]
#[path = "daemon_reports_tests.rs"]
mod tests;
