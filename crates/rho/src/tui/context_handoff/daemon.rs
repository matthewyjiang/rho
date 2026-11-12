//! The loaded-session handoff for the daemon session (#1667).
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
//! session may switch to it, to compact on that model and then continue on
//! this one, and to continue. "Compact, then continue" switches the session
//! to the source model, sends `/compact`, and has the daemon switch back
//! once that turn ends having compacted. A compaction that fails, leaves the
//! conversation as it was, or is cancelled keeps the session on the source
//! model.

use rho_protocol::{ModelRef, SessionChange, SessionReport};
use rho_providers::{model::catalog::ModelSelection, provider::model_reference};

use super::{
    decision_from_value, selection_matches_runtime, ContextHandoffDecision, ContextHandoffImpact,
    PendingContextHandoff,
};
use crate::app::provider_context_report::ProviderContextReport;
use crate::commands::CommandId;
use crate::tui::{
    daemon_events::ReportRefusal, App, ComposerMode, DefaultTerminal, Entry,
    InteractiveModelSelection, TurnPrompt,
};

impl App {
    /// Asks the daemon what the followed session's model cannot be sent, when
    /// the session has a saved conversation; the answer opens the dialog
    /// ([`Self::show_daemon_provider_context`]).
    pub(in crate::tui) fn request_daemon_context_handoff(&mut self) {
        let session = &self.info.session.daemon;
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
        // The Auto gate waited for this answer; a handoff that opened runs
        // it once resolved.
        if let Err(error) = self.reconcile_auto_classifier_gate() {
            self.insert_entry(&Entry::Error(format!(
                "could not change permission mode: {error:#}"
            )));
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
        // Only a model the session may switch to, as the daemon says.
        let source_selection = report
            .source
            .as_ref()
            .filter(|_| report.source_selectable)
            .map(|source| daemon_selection(self, &source.provider, &source.model));
        let impact = ContextHandoffImpact {
            source_label: source.unwrap_or_else(|| "session model".into()),
            target_label: model_reference(&self.info.runtime.provider, &self.info.runtime.model),
            omissions,
            // Compacting summarizes on the source model.
            can_compact: source_selection.is_some()
                && report.can_compact
                && self.daemon_offers_built_in(CommandId::Compact),
            offer_use_source: source_selection.is_some(),
        };
        let choice = match impact.choice() {
            Ok(choice) => choice,
            Err(error) => {
                tracing::warn!(%error, "context handoff choice unavailable");
                return;
            }
        };
        self.open_context_handoff(choice, PendingContextHandoff { source_selection });
    }

    /// Applies the person's answer: the session already runs on the model
    /// the dialog named, so continuing changes nothing. The daemon's options
    /// say where a switch landed, or a row why not.
    pub(super) async fn resolve_daemon_context_handoff(
        &mut self,
        value: Option<&str>,
        source_selection: Option<InteractiveModelSelection>,
        terminal: &mut DefaultTerminal,
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
                self.configure_daemon(model_change(
                    &source.selection.provider,
                    &source.selection.model,
                    /*auth*/ None,
                ))
            }
            Some(ContextHandoffDecision::CompactThenContinue) => {
                self.compact_then_continue_on_daemon(source_selection, terminal)
                    .await
            }
            Some(ContextHandoffDecision::ContinueDirect) | None => Ok(()),
        }
    }

    /// Compacts on `source`, then continues on the model and auth the
    /// session runs on now. Once `/compact` started, the
    /// switch back goes to the daemon as a
    /// [`SessionChange::ModelAfterCompaction`]: the daemon applies it only if
    /// the compaction reduced the conversation, whether or not this TUI still
    /// follows the session, and a change sent meanwhile applies after it. A
    /// compaction that does not compact, or never starts, keeps the session
    /// on `source`. One that cannot start while the session's MCP servers
    /// connect changes nothing.
    async fn compact_then_continue_on_daemon(
        &mut self,
        source: Option<InteractiveModelSelection>,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.daemon_tool_sources_loading() {
            // The daemon would start it only once they connected, and the
            // TUI would wait for it meanwhile.
            self.insert_entry(&Entry::Error(
                "could not compact before continuing: the session's MCP servers are still connecting"
                    .into(),
            ));
            return Ok(());
        }
        let runtime = &self.info.runtime;
        let target = SessionChange::ModelAfterCompaction {
            model: ModelRef {
                provider: runtime.provider.clone(),
                model: runtime.model.clone(),
            },
            auth: Some(runtime.auth.clone()),
        };
        let switched = source.filter(|source| !selection_matches_runtime(self, source));
        if let Some(source) = &switched {
            self.configure_daemon(model_change(
                &source.selection.provider,
                &source.selection.model,
                /*auth*/ None,
            ))?;
        }
        let compact = TurnPrompt::command("/compact".into(), "/compact".into());
        match self.request_daemon_turn(&compact, &[]).await {
            Ok(()) if switched.is_some() => {
                self.configure_daemon(target)?;
                // Before the views, which may hold the compaction's end.
                self.daemon_compact_handoff = true;
            }
            Ok(()) => {}
            Err(error) => {
                let ended = error
                    .downcast_ref::<rho_protocol::ClientError>()
                    .and_then(rho_protocol::ClientError::turn_ended);
                // A compaction that ended before it began has the daemon's
                // own rows to say how.
                if ended.is_none() {
                    self.insert_entry(&Entry::Error(format!(
                        "could not compact before continuing: {error:#}"
                    )));
                }
                if switched.is_some() {
                    self.insert_entry(&Entry::Notice(KEPT_SOURCE.into()));
                }
            }
        }
        self.show_taken_daemon_views(terminal).await
    }

    /// The `/compact` of "Compact, then continue" ended as this TUI saw it:
    /// `compacted` when its compaction reduced the conversation, `None` when
    /// the TUI did not see how it ended. Says when the daemon kept the
    /// source model; does nothing when no handoff waits.
    pub(in crate::tui) fn settle_daemon_compact_handoff(&mut self, compacted: Option<bool>) {
        if std::mem::take(&mut self.daemon_compact_handoff) && compacted == Some(false) {
            self.insert_entry(&Entry::Notice(KEPT_SOURCE.into()));
        }
    }
}

/// The notice for a "Compact, then continue" that did not compact.
const KEPT_SOURCE: &str = "kept the session model because context was not compacted";

/// The daemon change that switches the session to `provider`/`model` on
/// `auth`, or on the auth the daemon chooses.
fn model_change(provider: &str, model: &str, auth: Option<&str>) -> SessionChange {
    SessionChange::Model {
        model: ModelRef {
            provider: provider.to_owned(),
            model: model.to_owned(),
        },
        auth: auth.map(str::to_owned),
        setup: false,
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
    }
}
