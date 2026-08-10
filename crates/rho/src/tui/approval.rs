use crossterm::event::{KeyCode, KeyEvent};
use rho_sdk::{ApprovalDecision, PendingApproval};

use super::{App, ComposerMode, HerdrUserWait};

mod render;

pub(in crate::tui) use render::approval_lines;
use render::{approval_detail_line_count, approval_detail_page_lines};

const DENIED_BY_USER_REASON: &str = "denied by user";
const CANCELLED_BY_USER_REASON: &str = "cancelled by user";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ApprovalKeyOutcome {
    Ignored,
    Handled,
    Resolved,
}

/// Fail-closed choice set for the supervised approval prompt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ApprovalChoice {
    AllowOnce,
    AllowForSession,
    #[default]
    Deny,
}

impl ApprovalChoice {
    pub(super) const ALL: [Self; 3] = [Self::AllowOnce, Self::AllowForSession, Self::Deny];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::AllowOnce => "Allow once",
            Self::AllowForSession => "Allow for session",
            Self::Deny => "Deny",
        }
    }

    pub(super) fn decision(self) -> ApprovalDecision {
        match self {
            Self::AllowOnce => ApprovalDecision::AllowOnce,
            Self::AllowForSession => ApprovalDecision::AllowForSession,
            Self::Deny => ApprovalDecision::Deny {
                reason: DENIED_BY_USER_REASON.into(),
            },
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::AllowOnce => Self::AllowOnce,
            Self::AllowForSession => Self::AllowOnce,
            Self::Deny => Self::AllowForSession,
        }
    }

    pub(super) const fn next(self) -> Self {
        match self {
            Self::AllowOnce => Self::AllowForSession,
            Self::AllowForSession => Self::Deny,
            Self::Deny => Self::Deny,
        }
    }
}

#[derive(Debug)]
pub(super) struct ApprovalComposer {
    pending: PendingApproval,
    active: ApprovalChoice,
    /// First visible detail line, measured from the start of the request.
    detail_offset: usize,
}

impl ApprovalComposer {
    fn new(pending: PendingApproval) -> Self {
        Self {
            pending,
            active: ApprovalChoice::Deny,
            detail_offset: 0,
        }
    }

    pub(super) fn request(&self) -> &rho_sdk::ApprovalRequest {
        self.pending.request()
    }

    pub(super) fn active(&self) -> ApprovalChoice {
        self.active
    }

    pub(super) fn detail_offset(&self) -> usize {
        self.detail_offset
    }

    fn scroll_details_by(&mut self, delta_pages: isize, width: usize, viewport_height: usize) {
        let page_lines = approval_detail_page_lines(viewport_height).max(1);
        let detail_len = approval_detail_line_count(self.request(), width);
        let max_offset = detail_len.saturating_sub(page_lines);
        let current = self.detail_offset.min(max_offset);
        let delta_lines = delta_pages.saturating_mul(page_lines as isize);
        self.detail_offset = if delta_lines < 0 {
            current.saturating_sub(delta_lines.unsigned_abs())
        } else {
            current.saturating_add(delta_lines as usize).min(max_offset)
        };
    }

    fn move_previous(&mut self) {
        self.active = self.active.previous();
    }

    fn move_next(&mut self) {
        self.active = self.active.next();
    }

    fn respond(&mut self, decision: ApprovalDecision) {
        let _ = self.pending.respond(decision);
    }
}

impl App {
    pub(super) async fn open_approval(&mut self, pending: PendingApproval) {
        self.input_ui
            .set_composer(ComposerMode::Approval(ApprovalComposer::new(pending)));
        self.set_status("approval requested");
        self.report_herdr_waiting_for_user(HerdrUserWait::Approval)
            .await;
    }

    pub(super) fn handle_approval_key(
        &mut self,
        key: KeyEvent,
        width: usize,
        viewport_height: usize,
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
                    approval.scroll_details_by(-1, width, viewport_height);
                }
                ApprovalKeyOutcome::Handled
            }
            KeyCode::PageDown => {
                if let ComposerMode::Approval(approval) = self.input_ui.composer_mut() {
                    approval.scroll_details_by(1, width, viewport_height);
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
        self.input_ui.clear_paste_burst();
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
            let decision = decision.unwrap_or_else(|| approval.active.decision());
            approval.respond(decision);
            self.set_status("running");
        }
    }
}

#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;
