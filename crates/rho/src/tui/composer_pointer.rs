//! Pointer input for composer modes that paint clickable choices.
//!
//! Questionnaire, approval, and inline choice renderers report which composer
//! lines hold which choice through
//! [`ComposerFrame::choice_hits`](super::view_composer::ComposerFrame).
//! A press is mapped onto those lines where the composer was painted: one
//! click selects, a double click confirms the way Enter does. Presses that miss
//! every choice fall through to the screen handler for selection and copy.

use std::{ops::Range, time::Instant};

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{
    approval::ApprovalChoice, frame_context::FrameContext, mouse::COMPOSER_DOUBLE_CLICK,
    questionnaire::QuestionnaireTarget, App,
};

/// A clickable span of composer lines.
///
/// Line indices count from the first composer line, before the visible-start
/// skip, so they match the renderer's own line numbering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ComposerHit<T> {
    pub(super) lines: Range<usize>,
    pub(super) columns: Range<usize>,
    pub(super) target: T,
}

impl<T> ComposerHit<T> {
    /// A target covering every column of `lines`.
    pub(super) fn rows(lines: Range<usize>, target: T) -> Self {
        Self {
            lines,
            columns: 0..usize::MAX,
            target,
        }
    }

    pub(super) fn map_target<U>(self, map: impl FnOnce(T) -> U) -> ComposerHit<U> {
        ComposerHit {
            lines: self.lines,
            columns: self.columns,
            target: map(self.target),
        }
    }
}

/// A choice painted by a questionnaire, approval, or inline choice composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComposerChoice {
    Questionnaire(QuestionnaireTarget),
    Approval(ApprovalChoice),
    /// Index of an available option in the open inline choice.
    InlineChoice(usize),
}

/// Whether a press on a choice selects it or confirms it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChoiceClick {
    Single,
    Double,
}

/// Resolves the target painted under a pointer. `origin` is the composer rect
/// and `start` the first composer line painted into it.
pub(super) fn composer_target_at<T: Copy>(
    hits: &[ComposerHit<T>],
    origin: Rect,
    start: usize,
    column: u16,
    row: u16,
) -> Option<T> {
    if !origin.contains(Position { x: column, y: row }) {
        return None;
    }
    let line = start.saturating_add(usize::from(row - origin.y));
    let column = usize::from(column - origin.x);
    hits.iter()
        .find(|hit| hit.lines.contains(&line) && hit.columns.contains(&column))
        .map(|hit| hit.target)
}

impl App {
    /// Handles a left press on a questionnaire, approval, or inline choice
    /// option painted in `ctx`. `false` when the event is not such a press, so
    /// the screen handler still scrolls, selects, and copies around the form.
    ///
    /// Clicks are ignored until the form has been painted, so a press that
    /// races a newly opened approval or destructive confirmation cannot move
    /// its focus. A single click only moves the highlight; resolving takes a
    /// double click on the same choice, like pressing Enter.
    pub(super) fn handle_choice_composer_mouse(
        &mut self,
        kind: MouseEventKind,
        ctx: &FrameContext,
        column: u16,
        row: u16,
        now: Instant,
    ) -> bool {
        if kind != MouseEventKind::Down(MouseButton::Left) || !self.input_ui.composer_painted() {
            return false;
        }
        let Some(target) = composer_target_at(
            &ctx.composer.choice_hits,
            ctx.layout.composer,
            ctx.layout.composer_start,
            column,
            row,
        ) else {
            return false;
        };

        self.history.clear_text_selection();
        self.screen_selection = None;
        self.ctrl_c_streak = 0;
        // Only confirmable choices form click sequences. The index keeps a
        // double click from spanning two choices that share a painted cell
        // across a relayout.
        let sequence_index = match target {
            ComposerChoice::Questionnaire(QuestionnaireTarget::Choice { index, confirms }) => {
                confirms.then_some(index)
            }
            ComposerChoice::Questionnaire(QuestionnaireTarget::Question(_)) => None,
            ComposerChoice::Approval(choice) => Some(choice as usize),
            ComposerChoice::InlineChoice(index) => Some(index),
        };
        let click = match sequence_index {
            Some(index)
                if self.input_ui.register_pointer_click(
                    now,
                    column,
                    row,
                    index,
                    COMPOSER_DOUBLE_CLICK,
                ) =>
            {
                ChoiceClick::Double
            }
            Some(_) => ChoiceClick::Single,
            None => {
                self.input_ui.cancel_pointer_click_sequence();
                ChoiceClick::Single
            }
        };
        match target {
            ComposerChoice::Questionnaire(target) => self.click_questionnaire(target, click),
            ComposerChoice::Approval(choice) => self.click_approval_choice(choice, click),
            ComposerChoice::InlineChoice(index) => self.click_inline_choice(index, click),
        }
        true
    }
}

#[cfg(test)]
#[path = "composer_pointer_tests.rs"]
mod tests;
