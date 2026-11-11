//! The loaded-session handoff for a daemon session (#1667).
//!
//! The daemon recovers the conversation, so the TUI asks it what the
//! session's model cannot be sent once it follows a saved session, at launch
//! and after `/resume`, and opens the dialog when the answer says some of it
//! will be omitted. The answer arrives as a report
//! (`SessionReport::ProviderContext`); a session in a turn refuses the read,
//! and the send gate (`send_confirm::daemon`) asks before its next prompt
//! instead.
//!
//! The dialog offers the model that produced the native context when the
//! session may switch to it, and to continue. It does not offer to compact
//! first, as in process: that summarizes on the source model and returns to
//! this one, which the daemon has no single request for yet. The send gate
//! still offers to compact before a send.

use rho_protocol::{ModelRef, SessionChange, SessionReport};
use rho_providers::{model::catalog::ModelSelection, provider::model_reference};

use super::{
    decision_from_value, ContextHandoffDecision, ContextHandoffImpact, ContextHandoffKind,
    PendingContextHandoff,
};
use crate::app::provider_context_report::ProviderContextReport;
use crate::tui::{
    daemon_events::ReportRefusal, reasoning_metadata::ModelSwitchReasoningPolicy, App,
    ComposerMode, Entry, InteractiveModelSelection,
};

impl App {
    /// Asks the daemon what the followed session's model cannot be sent, when
    /// the session has a saved conversation; the answer opens the dialog
    /// ([`Self::show_daemon_provider_context`]). Does nothing in process.
    pub(in crate::tui) fn request_daemon_context_handoff(&mut self) {
        let Some(session) = self.info.session.daemon.as_ref() else {
            return;
        };
        if session.saved_id().is_none() {
            return;
        }
        match session.inspect(SessionReport::ProviderContext) {
            // A prompt or a switch to another session stops the wait, so an
            // answer that arrives after the person moved on opens nothing.
            Ok(()) => self.awaited_daemon_report = Some(SessionReport::ProviderContext),
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not read provider context: {error}"
            ))),
        }
    }

    /// The daemon answered [`Self::request_daemon_context_handoff`].
    pub(in crate::tui) fn show_daemon_provider_context(
        &mut self,
        result: Result<serde_json::Value, ReportRefusal>,
    ) {
        if self.awaited_daemon_report != Some(SessionReport::ProviderContext) {
            return;
        }
        self.awaited_daemon_report = None;
        let report = result
            .map_err(|refusal| (!refusal.busy).then_some(refusal.message))
            .and_then(|value| {
                serde_json::from_value::<ProviderContextReport>(value)
                    .map_err(|error| Some(error.to_string()))
            });
        match report {
            Ok(report) => self.offer_daemon_context_handoff(&report),
            // In a turn: the send gate asks before the next prompt.
            Err(None) => {}
            Err(Some(error)) => self.insert_entry(&Entry::Error(format!(
                "could not read provider context: {error}"
            ))),
        }
    }

    fn offer_daemon_context_handoff(&mut self, report: &ProviderContextReport) {
        // A late answer must not replace what the person opened meanwhile,
        // nor open over a turn another client started: the send gate asks
        // before this TUI's next prompt.
        if !matches!(self.input_ui.composer(), ComposerMode::Input) || self.daemon_turn_runs() {
            return;
        }
        let omissions = report.omissions();
        if !omissions.has_omissions() {
            return;
        }
        let source = report
            .source
            .as_ref()
            .map(|source| model_reference(&source.provider, &source.model));
        // Only a model the session may switch to, as the daemon lists them.
        let source_selection = report.source.as_ref().and_then(|source| {
            self.daemon_options
                .as_ref()?
                .model
                .available
                .iter()
                .any(|choice| {
                    choice.model.provider == source.provider && choice.model.model == source.model
                })
                .then(|| daemon_selection(self, &source.provider, &source.model))
        });
        let impact = ContextHandoffImpact {
            source_label: source.unwrap_or_else(|| "session model".into()),
            target_label: model_reference(&self.info.runtime.provider, &self.info.runtime.model),
            omissions,
            can_compact: false,
            offer_use_source: source_selection.is_some(),
        };
        let choice = match impact.choice(ContextHandoffKind::LoadedSession) {
            Ok(choice) => choice,
            Err(error) => {
                tracing::warn!(%error, "context handoff choice unavailable");
                return;
            }
        };
        self.open_context_handoff(
            choice,
            PendingContextHandoff {
                kind: ContextHandoffKind::LoadedSession,
                source_selection,
                target_selection: None,
                materialize: None,
            },
        );
    }

    /// Applies the person's answer: the session already runs on the model
    /// the dialog named, so only a switch to the source model changes it.
    /// The daemon's options say where a switch landed, or a row why not.
    pub(super) fn resolve_daemon_context_handoff(
        &mut self,
        value: Option<&str>,
        source_selection: Option<InteractiveModelSelection>,
    ) -> anyhow::Result<()> {
        self.set_status("ready");
        let decision = value.map(|value| {
            decision_from_value(value).unwrap_or(ContextHandoffDecision::ContinueDirect)
        });
        match decision {
            Some(ContextHandoffDecision::UseSourceModel) => {
                let Some(source) = source_selection else {
                    self.insert_entry(&Entry::Error(
                        "model handoff failed: session model is unavailable".into(),
                    ));
                    return Ok(());
                };
                self.configure_daemon(SessionChange::Model {
                    model: ModelRef {
                        provider: source.selection.provider,
                        model: source.selection.model,
                    },
                    auth: None,
                    setup: false,
                })
            }
            // Not offered for a daemon session; see the module docs.
            Some(ContextHandoffDecision::CompactThenContinue)
            | Some(ContextHandoffDecision::ContinueDirect)
            | None => Ok(()),
        }
    }
}

/// `provider`/`model` as a selection the daemon session switches to. The
/// daemon chooses the auth, so the one this names goes unread.
fn daemon_selection(app: &App, provider: &str, model: &str) -> InteractiveModelSelection {
    InteractiveModelSelection {
        selection: ModelSelection {
            provider: provider.to_owned(),
            model: model.to_owned(),
            auth: app.info.runtime.auth.clone(),
            from_catalog: true,
        },
        alias: None,
        reasoning_policy: ModelSwitchReasoningPolicy::PreserveExplicit,
    }
}
