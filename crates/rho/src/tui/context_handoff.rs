//! Ask before provider-native context is omitted on a loaded session.
//!
//! Model switches apply immediately and surface an omission notice instead;
//! the send itself is confirmed by `send_confirm` before the next turn starts.
//! Compaction can salvage portable text. It does not make native blocks
//! sendable to an incompatible model.
//!
//! The daemon session holds the conversation, so the TUI asks from what the
//! daemon reports; see [`daemon`].

use crate::tui::DefaultTerminal;
use rho_sdk::model::handoff::HandoffReport;

use super::{
    App, ComposerMode, InlineChoice, InlineChoiceModal, InlineChoiceOption, InlineChoicePending,
    InteractiveModelSelection,
};

mod daemon;

pub(super) const ACTION_USE_SOURCE: &str = "use-source";
pub(super) const ACTION_COMPACT: &str = "compact";
pub(super) const ACTION_CONTINUE: &str = "continue";

#[derive(Debug)]
pub(super) struct PendingContextHandoff {
    /// The model that produced the native context, when the session may
    /// switch to it.
    source_selection: Option<InteractiveModelSelection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContextHandoffImpact {
    source_label: String,
    target_label: String,
    omissions: HandoffReport,
    can_compact: bool,
    offer_use_source: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextHandoffDecision {
    UseSourceModel,
    CompactThenContinue,
    ContinueDirect,
}

impl ContextHandoffImpact {
    fn choice(&self) -> anyhow::Result<InlineChoice> {
        InlineChoice::new(
            format!("How should Rho continue on {}?", self.target_label),
            self.description(),
            self.options(),
        )
    }

    fn description(&self) -> String {
        let kinds = self.omissions.omitted_kinds.join(", ");
        let blocks = self.omissions.omitted_provider_context;
        format!(
            "This conversation has {blocks} provider-native context block(s) ({kinds}). {} cannot use them. The transcript is already loaded. Compaction can summarize portable context first; it does not make native blocks sendable.",
            self.target_label
        )
    }

    fn options(&self) -> Vec<InlineChoiceOption> {
        let mut options = Vec::new();
        let mut shortcut = '1';

        if self.offer_use_source {
            options.push(InlineChoiceOption::available(
                ACTION_USE_SOURCE,
                shortcut,
                format!("Switch to {}", self.source_label),
                "Use the model that produced the native context so it can be replayed.",
            ));
            shortcut = next_shortcut(shortcut);
        }

        if self.can_compact {
            options.push(InlineChoiceOption::available(
                ACTION_COMPACT,
                shortcut,
                "Compact, then continue",
                format!(
                    "Summarize with {}, then continue. {} native block(s) still will not be sent to {}.",
                    self.source_label,
                    self.omissions.omitted_provider_context,
                    self.target_label
                ),
            ));
            shortcut = next_shortcut(shortcut);
        }

        options.push(InlineChoiceOption::available(
            ACTION_CONTINUE,
            shortcut,
            format!("Continue with {}", self.target_label),
            format!(
                "{} native block(s) will not be sent to {}. Transcript and tool history remain.",
                self.omissions.omitted_provider_context, self.target_label
            ),
        ));
        options
    }
}

impl App {
    /// Asks the daemon about the loaded session's provider context once the
    /// composer is free; the answer may open the handoff dialog.
    pub(super) fn maybe_offer_loaded_session_context_handoff(&mut self) {
        if matches!(self.input_ui.composer(), ComposerMode::Input) {
            self.request_daemon_context_handoff();
        }
    }

    fn open_context_handoff(&mut self, choice: InlineChoice, pending: PendingContextHandoff) {
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::ContextHandoff(Box::new(pending)),
                parent_picker: None,
            }));
        self.set_status("choose loaded-session handoff");
    }

    pub(super) async fn resolve_context_handoff(
        &mut self,
        value: Option<&str>,
        pending: PendingContextHandoff,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.resolve_daemon_context_handoff(value, pending.source_selection, terminal)
            .await?;
        // The handoff opens before the startup Auto gate and owns the
        // composer until resolved. Reconcile once the modal is gone so Auto
        // without a classifier cannot remain fail-closed.
        self.reconcile_auto_classifier_gate()
    }
}

fn decision_from_value(value: &str) -> Option<ContextHandoffDecision> {
    match value {
        ACTION_USE_SOURCE => Some(ContextHandoffDecision::UseSourceModel),
        ACTION_COMPACT => Some(ContextHandoffDecision::CompactThenContinue),
        ACTION_CONTINUE => Some(ContextHandoffDecision::ContinueDirect),
        _ => None,
    }
}

fn selection_matches_runtime(app: &App, selection: &InteractiveModelSelection) -> bool {
    selection.selection.provider == app.info.runtime.provider
        && selection.selection.model == app.info.runtime.model
        && selection.selection.auth == app.info.runtime.auth
}

fn next_shortcut(current: char) -> char {
    char::from_u32(u32::from(current) + 1).unwrap_or(current)
}

#[cfg(test)]
#[path = "context_handoff_tests.rs"]
mod tests;
