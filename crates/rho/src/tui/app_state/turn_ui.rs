//! Live-turn UI: provider attempt, activity, spinner, and in-flight tools.

use rho_sdk::ToolCallId;

use crate::tui::{
    activity::{ActivityPhase, LoadingSpinner},
    provider_attempt::ProviderAttempt,
    reasoning_phase::ReasoningPhase,
    tool_call_batch::ToolCallBatch,
    ToolEntry,
};

/// TUI session phase distinct from provider run controller state.
///
/// `ProviderTurn` should stay aligned with `InteractiveRuntime::is_run_active`
/// except for brief setup before `start` succeeds. `Compacting` is UI-only busy
/// work with no active provider run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) enum SessionUiPhase {
    #[default]
    Idle,
    ProviderTurn,
    Compacting,
}

impl SessionUiPhase {
    pub(in crate::tui) const fn is_busy(self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub(in crate::tui) const fn is_provider_turn(self) -> bool {
        matches!(self, Self::ProviderTurn)
    }

    pub(in crate::tui) const fn allows_idle_subagent_delivery(self) -> bool {
        matches!(self, Self::Idle)
    }

    pub(in crate::tui) const fn busy_status_label(self) -> &'static str {
        if self.is_busy() {
            "running"
        } else {
            "ready"
        }
    }
}

/// Live-turn UI: provider attempt, activity, spinner, and in-flight tools.
#[derive(Default)]
pub(in crate::tui) struct TurnUi {
    current_turn_start: Option<usize>,
    provider_attempt: ProviderAttempt,
    reasoning_phase: ReasoningPhase,
    session_ui: SessionUiPhase,
    activity_phase: ActivityPhase,
    loading_spinner: LoadingSpinner,
    tool_calls: ToolCallBatch,
}

impl TurnUi {
    pub(in crate::tui) fn current_turn_start(&self) -> Option<usize> {
        self.current_turn_start
    }

    pub(in crate::tui) fn set_current_turn_start(&mut self, start: Option<usize>) {
        self.current_turn_start = start;
    }

    pub(in crate::tui) fn provider_attempt_mut(&mut self) -> &mut ProviderAttempt {
        &mut self.provider_attempt
    }

    pub(in crate::tui) fn reasoning_phase(&self) -> &ReasoningPhase {
        &self.reasoning_phase
    }

    pub(in crate::tui) fn reasoning_phase_mut(&mut self) -> &mut ReasoningPhase {
        &mut self.reasoning_phase
    }

    pub(in crate::tui) fn session_ui(&self) -> SessionUiPhase {
        self.session_ui
    }

    pub(in crate::tui) fn is_busy(&self) -> bool {
        self.session_ui.is_busy()
    }

    pub(in crate::tui) fn is_provider_turn(&self) -> bool {
        self.session_ui.is_provider_turn()
    }

    pub(in crate::tui) fn enter_provider_turn(&mut self) {
        self.session_ui = SessionUiPhase::ProviderTurn;
    }

    pub(in crate::tui) fn enter_compact(&mut self) {
        self.session_ui = SessionUiPhase::Compacting;
    }

    pub(in crate::tui) fn end_busy(&mut self) {
        self.session_ui = SessionUiPhase::Idle;
    }

    pub(in crate::tui) fn activity_phase(&self) -> ActivityPhase {
        self.activity_phase
    }

    pub(in crate::tui) fn set_activity_phase(&mut self, phase: ActivityPhase) {
        self.activity_phase = phase;
    }

    pub(in crate::tui) fn loading_spinner(&self) -> &LoadingSpinner {
        &self.loading_spinner
    }

    pub(in crate::tui) fn start_loading(&mut self) {
        self.loading_spinner.start();
    }

    pub(in crate::tui) fn start_loading_if_needed(&mut self) {
        self.loading_spinner.start_if_needed();
    }

    pub(in crate::tui) fn stop_loading(&mut self) {
        self.loading_spinner.stop();
    }

    pub(in crate::tui) fn tool_calls(&self) -> &ToolCallBatch {
        &self.tool_calls
    }

    /// Mutable tool-batch access for map/preview surgery outside batch lifecycle.
    pub(in crate::tui) fn tool_calls_mut(&mut self) -> &mut ToolCallBatch {
        &mut self.tool_calls
    }

    pub(in crate::tui) fn clear_tool_calls(&mut self) {
        self.tool_calls.clear();
    }

    pub(in crate::tui) fn tool_started(&mut self, call_id: ToolCallId, display_lines: Vec<String>) {
        self.tool_calls.started(call_id, display_lines);
    }

    pub(in crate::tui) fn tool_updated(&mut self, call_id: ToolCallId, display_lines: Vec<String>) {
        self.tool_calls.updated(call_id, display_lines);
    }

    pub(in crate::tui) fn tool_call_preview(
        &mut self,
        index: usize,
        call_id: Option<ToolCallId>,
        display_lines: Vec<String>,
    ) {
        self.tool_calls.preview(index, call_id, display_lines);
    }

    pub(in crate::tui) fn tool_finished(&mut self, call_id: &ToolCallId) -> bool {
        self.tool_calls.finished(call_id)
    }

    pub(in crate::tui) fn latest_tool_mut(&mut self) -> Option<&mut ToolEntry> {
        self.tool_calls.latest_mut()
    }
}
