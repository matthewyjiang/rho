use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    picker_overlay::{picker_overlay_layout, OverlayPageTarget},
    App, InteractiveRuntime, UiPicker,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerKeyEffect {
    None,
    Handled,
    Submit,
    Escape,
    ToggleFavorite,
}

fn overlay_page_target(picker: &UiPicker, terminal: &DefaultTerminal) -> Option<OverlayPageTarget> {
    if !picker.is_overlay() {
        return None;
    }
    let size = terminal.size().ok()?;
    Some(
        picker_overlay_layout(
            Rect::new(0, 0, size.width, size.height),
            picker.has_item_details(),
        )
        .page_target(),
    )
}

fn apply_page_key(picker: &mut UiPicker, target: OverlayPageTarget, direction: isize) {
    match target {
        OverlayPageTarget::Detail(viewport) => {
            picker.scroll_detail_page(direction, viewport);
        }
        OverlayPageTarget::Nav { rows } => {
            picker.select_by_offset(direction.saturating_mul(rows as isize));
        }
    }
}

fn apply_home_end_key(picker: &mut UiPicker, target: OverlayPageTarget, home: bool) {
    match target {
        OverlayPageTarget::Detail(viewport) => {
            if home {
                picker.scroll_detail_home();
            } else {
                picker.scroll_detail_end(viewport);
            }
        }
        OverlayPageTarget::Nav { .. } => {
            if home {
                picker.select_first_match();
            } else {
                picker.select_last_match();
            }
        }
    }
}

fn apply_picker_key(
    picker: &mut UiPicker,
    key: KeyEvent,
    page_target: Option<OverlayPageTarget>,
    model_picker_open: bool,
    space_confirms: bool,
) -> PickerKeyEffect {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Up) => {
            picker.select_previous();
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            picker.select_next();
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::PageUp) => {
            if let Some(target) = page_target {
                apply_page_key(picker, target, -1);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::PageDown) => {
            if let Some(target) = page_target {
                apply_page_key(picker, target, 1);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Home) => {
            if let Some(target) = page_target {
                apply_home_end_key(picker, target, /*home*/ true);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::End) => {
            if let Some(target) = page_target {
                apply_home_end_key(picker, target, /*home*/ false);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Tab) => {
            picker.complete_filter();
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            picker.pop_filter_char();
            PickerKeyEffect::Handled
        }
        (KeyModifiers::CONTROL, KeyCode::Char('p')) if model_picker_open => {
            PickerKeyEffect::ToggleFavorite
        }
        (KeyModifiers::NONE, KeyCode::Char(' ')) if space_confirms => PickerKeyEffect::Submit,
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
            picker.push_filter_char(ch);
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Enter) => PickerKeyEffect::Submit,
        (_, KeyCode::Esc) => PickerKeyEffect::Escape,
        _ => PickerKeyEffect::None,
    }
}

impl App {
    pub(super) async fn handle_picker_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), super::ComposerMode::Picker(_)) {
            return Ok(false);
        }

        let model_picker_open = self.model_picker_is_open();
        let space_confirms = self.picker_space_confirms_selection();
        let effect = {
            let super::ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
                return Ok(false);
            };
            let page_target = overlay_page_target(picker, terminal);
            apply_picker_key(picker, key, page_target, model_picker_open, space_confirms)
        };

        match effect {
            PickerKeyEffect::None => Ok(true),
            PickerKeyEffect::Handled => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            PickerKeyEffect::Submit => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                self.submit_picker_selection(terminal, agent).await?;
                Ok(true)
            }
            PickerKeyEffect::Escape => {
                self.handle_picker_escape(/*running*/ false)?;
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            PickerKeyEffect::ToggleFavorite => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
        }
    }

    pub(super) fn handle_running_picker_key(
        &mut self,
        key: KeyEvent,
        terminal: &DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), super::ComposerMode::Picker(_)) {
            return Ok(false);
        }

        let model_picker_open = self.model_picker_is_open();
        let space_confirms = self.picker_space_confirms_selection();
        let effect = {
            let super::ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
                return Ok(false);
            };
            let page_target = overlay_page_target(picker, terminal);
            apply_picker_key(picker, key, page_target, model_picker_open, space_confirms)
        };

        match effect {
            PickerKeyEffect::None => Ok(true),
            PickerKeyEffect::Handled => Ok(true),
            PickerKeyEffect::Submit => {
                self.submit_picker_selection_during_turn()?;
                Ok(true)
            }
            PickerKeyEffect::Escape => {
                self.handle_picker_escape(/*running*/ true)?;
                Ok(true)
            }
            PickerKeyEffect::ToggleFavorite => {
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
        }
    }
}
