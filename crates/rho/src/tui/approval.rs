use crossterm::event::{KeyCode, KeyEvent};
use rho_sdk::ApprovalDecision;
use tokio::sync::oneshot;

use super::{composer_pointer::ChoiceClick, App, ComposerMode, UserWait};

mod daemon_ask;
mod render;

use render::{approval_detail_line_count, approval_detail_page_lines};
pub(in crate::tui) use render::{approval_frame, ApprovalPrompt};

const CANCELLED_BY_USER_REASON: &str = "cancelled by user";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ApprovalKeyOutcome {
    Ignored,
    Handled,
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
                reason: crate::permission::DENIED_BY_USER.into(),
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
    prompt: ApprovalPrompt,
    /// Where the decision goes. Dropped unanswered, it says nothing was
    /// decided.
    reply: Option<oneshot::Sender<ApprovalDecision>>,
    active: ApprovalChoice,
    /// First visible detail line, measured from the start of the request.
    detail_offset: usize,
    /// Reopen `/side` after this prompt if it was on screen when approval opened.
    restore_side: bool,
}

impl ApprovalComposer {
    fn new(
        prompt: ApprovalPrompt,
        reply: oneshot::Sender<ApprovalDecision>,
        restore_side: bool,
    ) -> Self {
        Self {
            prompt,
            reply: Some(reply),
            active: ApprovalChoice::Deny,
            detail_offset: 0,
            restore_side,
        }
    }

    pub(super) fn prompt(&self) -> &ApprovalPrompt {
        &self.prompt
    }

    pub(super) fn active(&self) -> ApprovalChoice {
        self.active
    }

    pub(super) fn detail_offset(&self) -> usize {
        self.detail_offset
    }

    fn scroll_details_by(&mut self, delta_pages: isize, width: usize, viewport_height: usize) {
        let page_lines = approval_detail_page_lines(viewport_height).max(1);
        let detail_len = approval_detail_line_count(&self.prompt, width);
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

    fn set_active(&mut self, choice: ApprovalChoice) {
        self.active = choice;
    }

    fn respond(&mut self, decision: ApprovalDecision) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(decision);
        }
    }
}

impl App {
    /// Opens the approval prompt for `prompt`, whose decision goes to
    /// `reply`.
    pub(super) async fn open_replying_approval(
        &mut self,
        prompt: ApprovalPrompt,
        reply: oneshot::Sender<ApprovalDecision>,
    ) {
        let restore_side = self.side_overlay_open();
        self.input_ui
            .set_composer(ComposerMode::Approval(ApprovalComposer::new(
                prompt,
                reply,
                restore_side,
            )));
        self.set_status("approval requested");
        self.wait_for_user(UserWait::Approval).await;
    }

    /// Closes the approval prompt undecided, as when its request closed
    /// elsewhere; `/side` comes back if the prompt replaced it.
    pub(super) fn withdraw_approval(&mut self) {
        match self.input_ui.take_composer() {
            ComposerMode::Approval(approval) => {
                if approval.restore_side && self.side_chat.is_some() {
                    self.input_ui.set_composer(ComposerMode::Side);
                }
            }
            composer => self.input_ui.set_composer(composer),
        }
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
                ApprovalKeyOutcome::Handled
            }
            KeyCode::Esc => {
                self.finish_approval(Some(ApprovalDecision::Deny {
                    reason: CANCELLED_BY_USER_REASON.into(),
                }));
                ApprovalKeyOutcome::Handled
            }
            _ => ApprovalKeyOutcome::Handled,
        };
        self.input_ui.clear_paste_burst();
        self.ctrl_c_streak = 0;
        Ok(outcome)
    }

    /// A click highlights a choice; a double click confirms it like Enter.
    pub(super) fn click_approval_choice(&mut self, choice: ApprovalChoice, click: ChoiceClick) {
        let ComposerMode::Approval(approval) = self.input_ui.composer_mut() else {
            return;
        };
        approval.set_active(choice);
        if click == ChoiceClick::Double {
            self.finish_approval(None);
        }
    }

    fn finish_approval(&mut self, decision: Option<ApprovalDecision>) {
        let composer = self.input_ui.take_composer();
        let restore_side = if let ComposerMode::Approval(mut approval) = composer {
            let decision = decision.unwrap_or_else(|| approval.active.decision());
            approval.respond(decision);
            self.turn.mark_approval_resolved();
            self.set_status("running");
            approval.restore_side
        } else {
            false
        };
        if restore_side && self.side_chat.is_some() {
            self.input_ui.set_composer(ComposerMode::Side);
        }
    }
}

#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;
