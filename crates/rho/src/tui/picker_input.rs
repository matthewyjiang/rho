use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    picker_overlay::{picker_overlay_body, picker_overlay_layout, DetailViewport, OverlayBody},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayKeyViewport {
    Detail(DetailViewport),
    Nav { rows: usize },
}

fn overlay_key_viewport(
    picker: &UiPicker,
    terminal: &DefaultTerminal,
) -> Option<OverlayKeyViewport> {
    if !picker.is_overlay() {
        return None;
    }
    let size = terminal.size().ok()?;
    let body = picker_overlay_body(picker);
    let layout = picker_overlay_layout(Rect::new(0, 0, size.width, size.height), body);
    Some(match body {
        OverlayBody::NavAndDetail => OverlayKeyViewport::Detail(layout.detail_viewport()),
        OverlayBody::NavOnly => OverlayKeyViewport::Nav {
            rows: layout.nav_viewport_rows.max(1),
        },
    })
}

fn apply_picker_key(
    picker: &mut UiPicker,
    key: KeyEvent,
    viewport: Option<OverlayKeyViewport>,
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
            match viewport {
                Some(OverlayKeyViewport::Detail(viewport)) => {
                    picker.scroll_detail_page(-1, viewport);
                }
                Some(OverlayKeyViewport::Nav { rows }) => {
                    picker.select_by_offset(-(rows as isize));
                }
                None => {}
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::PageDown) => {
            match viewport {
                Some(OverlayKeyViewport::Detail(viewport)) => {
                    picker.scroll_detail_page(1, viewport);
                }
                Some(OverlayKeyViewport::Nav { rows }) => {
                    picker.select_by_offset(rows as isize);
                }
                None => {}
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Home) => {
            match viewport {
                Some(OverlayKeyViewport::Detail(_)) => picker.scroll_detail_home(),
                Some(OverlayKeyViewport::Nav { .. }) => picker.select_first_match(),
                None => {}
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::End) => {
            match viewport {
                Some(OverlayKeyViewport::Detail(viewport)) => {
                    picker.scroll_detail_end(viewport);
                }
                Some(OverlayKeyViewport::Nav { .. }) => picker.select_last_match(),
                None => {}
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
        if !matches!(self.composer, super::ComposerMode::Picker(_)) {
            return Ok(false);
        }

        let model_picker_open = self.model_picker_is_open();
        let space_confirms = self.picker_space_confirms_selection();
        let viewport = {
            let super::ComposerMode::Picker(picker) = &self.composer else {
                return Ok(false);
            };
            overlay_key_viewport(picker, terminal)
        };
        let effect = {
            let super::ComposerMode::Picker(picker) = &mut self.composer else {
                return Ok(false);
            };
            apply_picker_key(picker, key, viewport, model_picker_open, space_confirms)
        };

        match effect {
            PickerKeyEffect::None => Ok(true),
            PickerKeyEffect::Handled => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            PickerKeyEffect::Submit => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit_picker_selection(terminal, agent).await?;
                Ok(true)
            }
            PickerKeyEffect::Escape => {
                self.handle_picker_escape(/*running*/ false)?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            PickerKeyEffect::ToggleFavorite => {
                self.paste_burst.clear();
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
        if !matches!(self.composer, super::ComposerMode::Picker(_)) {
            return Ok(false);
        }

        let model_picker_open = self.model_picker_is_open();
        let space_confirms = self.picker_space_confirms_selection();
        let viewport = {
            let super::ComposerMode::Picker(picker) = &self.composer else {
                return Ok(false);
            };
            overlay_key_viewport(picker, terminal)
        };
        let effect = {
            let super::ComposerMode::Picker(picker) = &mut self.composer else {
                return Ok(false);
            };
            apply_picker_key(picker, key, viewport, model_picker_open, space_confirms)
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
