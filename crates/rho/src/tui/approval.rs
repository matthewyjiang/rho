use crossterm::event::{KeyCode, KeyEvent};
use rho_sdk::{ApprovalDecision, PendingApproval};

use super::{App, ComposerMode, HerdrUserWait};

mod render;

use render::approval_detail_page_count;
pub(in crate::tui) use render::approval_lines;

const APPROVAL_CHOICE_COUNT: usize = 3;
const DENIED_BY_USER_REASON: &str = "denied by user";
const CANCELLED_BY_USER_REASON: &str = "cancelled by user";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ApprovalKeyOutcome {
    Ignored,
    Handled,
    Resolved,
}

#[derive(Debug)]
pub(super) struct ApprovalComposer {
    pending: PendingApproval,
    active: usize,
    detail_pages_before_end: usize,
}

impl ApprovalComposer {
    fn new(pending: PendingApproval) -> Self {
        Self {
            pending,
            active: 0,
            detail_pages_before_end: 0,
        }
    }

    pub(super) fn request(&self) -> &rho_sdk::ApprovalRequest {
        self.pending.request()
    }

    pub(super) fn active(&self) -> usize {
        self.active
    }

    pub(super) fn detail_pages_before_end(&self) -> usize {
        self.detail_pages_before_end
    }

    fn scroll_details_up(&mut self, width: usize) {
        let page_count = approval_detail_page_count(self.request(), width);
        self.detail_pages_before_end = self
            .detail_pages_before_end
            .saturating_add(1)
            .min(page_count.saturating_sub(1));
    }

    fn scroll_details_down(&mut self) {
        self.detail_pages_before_end = self.detail_pages_before_end.saturating_sub(1);
    }

    fn move_previous(&mut self) {
        self.active = previous_choice(self.active);
    }

    fn move_next(&mut self) {
        self.active = next_choice(self.active);
    }

    fn respond(&mut self, decision: ApprovalDecision) {
        let _ = self.pending.respond(decision);
    }
}

pub(super) fn previous_choice(active: usize) -> usize {
    active.saturating_sub(1)
}

pub(super) fn next_choice(active: usize) -> usize {
    (active + 1).min(APPROVAL_CHOICE_COUNT - 1)
}

pub(super) fn approval_decision(active: usize) -> ApprovalDecision {
    match active {
        0 => ApprovalDecision::AllowOnce,
        1 => ApprovalDecision::AllowForSession,
        _ => ApprovalDecision::Deny {
            reason: DENIED_BY_USER_REASON.into(),
        },
    }
}

impl App {
    pub(super) async fn open_approval(&mut self, pending: PendingApproval) {
        self.input_ui
            .set_composer(ComposerMode::Approval(ApprovalComposer::new(pending)));
        self.status = "approval requested".into();
        self.report_herdr_waiting_for_user(HerdrUserWait::Approval)
            .await;
    }

    pub(super) fn handle_approval_key(
        &mut self,
        key: KeyEvent,
        width: usize,
    ) -> anyhow::Result<ApprovalKeyOutcome> {
        if !matches!(self.input_ui.composer(), ComposerMode::Approval(_)) {
            return Ok(ApprovalKeyOutcome::Ignored);
        }

        let outcome = match key.code {
            KeyCode::Left | KeyCode::Up => {
                if let ComposerMode::Approval(approval) = self.input_ui.composer_mut() {
                    approval.move_previous();
                }
                ApprovalKeyOutcome::Handled
            }
            KeyCode::Right | KeyCode::Down => {
                if let ComposerMode::Approval(approval) = self.input_ui.composer_mut() {
                    approval.move_next();
                }
                ApprovalKeyOutcome::Handled
            }
            KeyCode::PageUp => {
                if let ComposerMode::Approval(approval) = self.input_ui.composer_mut() {
                    approval.scroll_details_up(width);
                }
                ApprovalKeyOutcome::Handled
            }
            KeyCode::PageDown => {
                if let ComposerMode::Approval(approval) = self.input_ui.composer_mut() {
                    approval.scroll_details_down();
                }
                ApprovalKeyOutcome::Handled
            }
            KeyCode::Enter => {
                self.finish_approval(None);
                ApprovalKeyOutcome::Resolved
            }
            KeyCode::Esc => {
                self.finish_approval(Some(ApprovalDecision::Deny {
                    reason: CANCELLED_BY_USER_REASON.into(),
                }));
                ApprovalKeyOutcome::Resolved
            }
            _ => ApprovalKeyOutcome::Handled,
        };
        self.input_ui.paste_burst_mut().clear();
        self.ctrl_c_streak = 0;
        Ok(outcome)
    }

    pub(super) fn cancel_approval(&mut self) {
        if matches!(self.input_ui.composer(), ComposerMode::Approval(_)) {
            self.finish_approval(Some(ApprovalDecision::Deny {
                reason: DENIED_BY_USER_REASON.into(),
            }));
        }
    }

    fn finish_approval(&mut self, decision: Option<ApprovalDecision>) {
        let composer = self.input_ui.take_composer();
        if let ComposerMode::Approval(mut approval) = composer {
            let decision = decision.unwrap_or_else(|| approval_decision(approval.active));
            approval.respond(decision);
            self.status = "running".into();
        }
    }
}

#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;
