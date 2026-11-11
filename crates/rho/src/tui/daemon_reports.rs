//! `/context` and `/hooks` against a daemon session: the daemon reads the
//! report from the session it owns, and the overlay opens when it answers.
//!
//! An answer opens its overlay only while the person still waits on it:
//! [`App::awaited_daemon_report`] names the report asked for last, and a
//! prompt or a switch to another session stops the wait, so an answer that
//! arrives after the person moved on is dropped.
//!
//! `/info` reads its Compaction section the same way, into the overlay it
//! already opened: no one waits on that answer, so it changes no status and
//! is dropped once the overlay closed. The daemon refuses the read during a
//! turn, so [`InfoCompactionRead`] asks again when the turn ends.
//!
//! The provider-context report a resumed session is asked for opens the
//! handoff dialog (`context_handoff::daemon`); no one waits on it either.

use rho_protocol::SessionReport;

use super::{daemon_events::ReportRefusal, App, ComposerMode, Entry, PanelOverlay};

/// Where the open `/info` overlay's read of a daemon session's Compaction
/// section stands. Each step returns the next state and whether to ask the
/// daemon now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum InfoCompactionRead {
    /// Nothing waits for the section.
    #[default]
    Idle,
    /// Asked. `turn_rested` once a turn ended since: a refusal as busy is
    /// then of that turn, and the read can go again at once.
    Asked { turn_rested: bool },
    /// The session is in a turn: asked when it ends.
    AwaitingRest,
}

impl InfoCompactionRead {
    fn opened(in_turn: bool) -> (Self, bool) {
        if in_turn {
            (Self::AwaitingRest, false)
        } else {
            (Self::Asked { turn_rested: false }, true)
        }
    }

    fn turn_rested(self) -> (Self, bool) {
        match self {
            Self::Idle => (Self::Idle, false),
            Self::Asked { .. } => (Self::Asked { turn_rested: true }, false),
            Self::AwaitingRest => (Self::Asked { turn_rested: false }, true),
        }
    }

    fn refused_as_busy(self) -> (Self, bool) {
        match self {
            Self::Asked { turn_rested: true } => (Self::Asked { turn_rested: false }, true),
            Self::Asked { turn_rested: false } | Self::AwaitingRest => (Self::AwaitingRest, false),
            Self::Idle => (Self::Idle, false),
        }
    }
}

/// What the person asked to see, for the status line and a refusal.
fn name(report: SessionReport) -> &'static str {
    match report {
        SessionReport::Context => "context",
        SessionReport::Hooks => "hooks",
        SessionReport::Compaction => "compaction",
        SessionReport::ProviderContext => "provider context",
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

    /// Reads the Compaction section of the `/info` overlay just opened on a
    /// daemon session: now, or once the turn under way ends.
    pub(super) fn open_daemon_info_compaction(&mut self) {
        if self.info.session.daemon.is_none() {
            return;
        }
        let step = InfoCompactionRead::opened(self.daemon_turn_runs());
        self.step_info_compaction(step);
    }

    /// A turn of the daemon session ended: a read that waited for it goes.
    pub(super) fn daemon_turn_rested_for_info(&mut self) {
        let step = self.info_compaction_read.turn_rested();
        self.step_info_compaction(step);
    }

    fn step_info_compaction(&mut self, (next, ask): (InfoCompactionRead, bool)) {
        // An overlay that closed, however it did, waits for nothing.
        if !matches!(
            self.input_ui.composer(),
            ComposerMode::Panel(PanelOverlay::Info(_))
        ) {
            self.info_compaction_read = InfoCompactionRead::Idle;
            return;
        }
        self.info_compaction_read = next;
        if !ask {
            return;
        }
        let Some(session) = self.info.session.daemon.as_ref() else {
            return;
        };
        if let Err(error) = session.inspect(SessionReport::Compaction) {
            self.info_compaction_read = InfoCompactionRead::Idle;
            self.apply_info_compaction(Err(error.to_string()));
        }
    }

    fn show_daemon_info_compaction(&mut self, result: Result<serde_json::Value, ReportRefusal>) {
        match result {
            Err(refusal) if refusal.busy => {
                let (next, ask) = self.info_compaction_read.refused_as_busy();
                // Until the read goes again the section says why it waits:
                // a job other than the session's turn may hold the session,
                // and no turn of this session need ever end.
                if !ask {
                    self.apply_info_compaction(Err(refusal.message));
                }
                self.step_info_compaction((next, ask));
            }
            result => {
                self.info_compaction_read = InfoCompactionRead::Idle;
                self.apply_info_compaction(result.map_err(|refusal| refusal.message).and_then(
                    |value| serde_json::from_value(value).map_err(|error| error.to_string()),
                ));
            }
        }
    }

    /// Opens the overlay for a report the daemon answered with.
    pub(super) fn show_daemon_report(
        &mut self,
        report: SessionReport,
        result: Result<serde_json::Value, ReportRefusal>,
    ) {
        match report {
            SessionReport::Compaction => return self.show_daemon_info_compaction(result),
            SessionReport::ProviderContext => return self.show_daemon_provider_context(result),
            _ => {}
        }
        let result = result.map_err(|refusal| refusal.message);
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
