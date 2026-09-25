//! Pointer input for composer modes that paint clickable choices.
//!
//! Questionnaire, approval, inline choice, and inline list picker renderers
//! report which composer lines hold which choice through
//! [`ComposerFrame::choice_hits`](super::view_composer::ComposerFrame).
//! A press is mapped onto those lines where the composer was painted: one
//! click selects, a double click confirms the way Enter does. Presses that miss
//! every choice fall through to the screen handler for selection and copy.
//!
//! Hover is resolved per frame from the last pointer cell against the same
//! hits (see [`App::refresh_pointer_hover`]), so it follows relayouts, typing,
//! and scrolling without waiting for the pointer to move.

use std::{ops::Range, time::Instant};

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{
    app_state::PointerAction,
    approval::ApprovalChoice,
    frame_context::FrameContext,
    mouse::COMPOSER_DOUBLE_CLICK,
    palette::{PaletteFrame, PaletteRow},
    questionnaire::QuestionnaireTarget,
    screen_layout::ScreenLayout,
    view_composer::ComposerFrame,
    App, ComposerMode,
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

/// A choice painted by a questionnaire, approval, inline choice, or inline
/// list picker composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComposerChoice {
    Questionnaire(QuestionnaireTarget),
    Approval(ApprovalChoice),
    /// Index of an available option in the open inline choice.
    InlineChoice(usize),
    /// An item row of the open inline (non-overlay) list picker.
    PickerRow(PickerRowTarget),
}

/// An item row painted by the inline list picker. Section headers get none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PickerRowTarget {
    /// Index into the picker's full item list, not the filtered matches.
    pub(super) item: usize,
    /// Row-space index of the first painted row. A click holds this window,
    /// so the rows do not shift under the pointer between two clicks.
    pub(super) window_start: usize,
}

/// What the pointer rests on in the composer and the palette above it, for
/// the hover lift. Both are `None` off every clickable row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PointerHover {
    pub(super) choice: Option<ComposerChoice>,
    pub(super) palette: Option<PaletteRow>,
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
    /// Handles a left press on a questionnaire, approval, inline choice, or
    /// inline list picker row painted in `ctx`. `false` when the event is not such a press, so
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
        // The setup screen paints its picker away from the session layout
        // these hits are mapped through.
        if kind != MouseEventKind::Down(MouseButton::Left)
            || !self.input_ui.composer_painted()
            || self.setup_step().is_some()
        {
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
            ComposerChoice::PickerRow(target) => Some(target.item),
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
            ComposerChoice::PickerRow(target) => self.click_inline_picker_row(target, click),
        }
        true
    }

    /// A click selects an inline picker row, holding the painted window; a
    /// double click also asks the event loop to submit it like Enter.
    fn click_inline_picker_row(&mut self, target: PickerRowTarget, click: ChoiceClick) {
        let ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
            return;
        };
        if picker.is_overlay() || !picker.select_item_in_window(target.item, target.window_start) {
            return;
        }
        self.preview_selected_theme_if_active();
        match click {
            ChoiceClick::Single => {}
            ChoiceClick::Double => self
                .input_ui
                .request_pointer_action(PointerAction::SubmitPicker),
        }
    }

    /// Resolves the composer choice and palette row under the last pointer
    /// cell against this frame's hits and layout, and records them for the
    /// hover lift. `true` only when the hovered targets changed, so the caller
    /// re-renders the rows with the new hover; hover restyles rows without
    /// changing their count, so the layout stays valid.
    ///
    /// The setup screen paints its composer elsewhere, so session geometry
    /// never hovers there.
    pub(super) fn refresh_pointer_hover(
        &mut self,
        composer: &ComposerFrame,
        palette: &PaletteFrame,
        layout: &ScreenLayout,
    ) -> bool {
        let hover = match self
            .last_mouse_position
            .filter(|_| self.setup_step().is_none())
        {
            Some((column, row)) => PointerHover {
                choice: composer_target_at(
                    &composer.choice_hits,
                    layout.composer,
                    layout.composer_start,
                    column,
                    row,
                ),
                palette: composer_target_at(
                    &palette.hits,
                    layout.commands,
                    /*start*/ 0,
                    column,
                    row,
                ),
            },
            None => PointerHover::default(),
        };
        self.input_ui.set_pointer_hover(hover)
    }
}

#[cfg(test)]
#[path = "composer_pointer_tests.rs"]
mod tests;
