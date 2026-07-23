//! Cohesive App-owned UI state groups for history, composer input, pending work,
//! and the live turn.

mod history_ui;
mod input_ui;
mod pending_work;
mod turn_ui;

pub(in crate::tui) use history_ui::HistoryUi;
pub(in crate::tui) use input_ui::InputUi;
pub(in crate::tui) use pending_work::PendingWorkUi;
#[cfg(test)]
pub(in crate::tui) use turn_ui::SessionUiPhase;
pub(in crate::tui) use turn_ui::TurnUi;
