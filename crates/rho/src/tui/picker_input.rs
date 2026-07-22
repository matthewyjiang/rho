use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{App, InteractiveRuntime};

fn navigable_popup_detail_metrics(terminal: &DefaultTerminal) -> (usize, usize) {
    match terminal.size() {
        Ok(size) => {
            super::render::navigable_picker_layout_metrics(Rect::new(0, 0, size.width, size.height))
        }
        Err(_) => (8, 40),
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

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.select_previous();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.select_next();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::PageUp) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    if picker.uses_navigable_popup() {
                        let (viewport, width) = navigable_popup_detail_metrics(terminal);
                        picker.scroll_detail_by(-(viewport.max(1) as isize));
                        picker.clamp_detail_scroll_for(width, viewport);
                    }
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::PageDown) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    if picker.uses_navigable_popup() {
                        let (viewport, width) = navigable_popup_detail_metrics(terminal);
                        picker.scroll_detail_by(viewport.max(1) as isize);
                        picker.clamp_detail_scroll_for(width, viewport);
                    }
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Home) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    if picker.uses_navigable_popup() {
                        picker.scroll_detail_home();
                    }
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::End) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    if picker.uses_navigable_popup() {
                        let (viewport, width) = navigable_popup_detail_metrics(terminal);
                        picker.scroll_detail_end();
                        picker.clamp_detail_scroll_for(width, viewport);
                    }
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.complete_filter();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.pop_filter_char();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::CONTROL, KeyCode::Char('p')) if self.model_picker_is_open() => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Char(' ')) if self.picker_space_confirms_selection() => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit_picker_selection(terminal, agent).await?;
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.push_filter_char(ch);
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit_picker_selection(terminal, agent).await?;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.handle_picker_escape(/*running*/ false)?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(true),
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

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.select_previous();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.select_next();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::PageUp) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    if picker.uses_navigable_popup() {
                        let (viewport, width) = navigable_popup_detail_metrics(terminal);
                        picker.scroll_detail_by(-(viewport.max(1) as isize));
                        picker.clamp_detail_scroll_for(width, viewport);
                    }
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::PageDown) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    if picker.uses_navigable_popup() {
                        let (viewport, width) = navigable_popup_detail_metrics(terminal);
                        picker.scroll_detail_by(viewport.max(1) as isize);
                        picker.clamp_detail_scroll_for(width, viewport);
                    }
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Home) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    if picker.uses_navigable_popup() {
                        picker.scroll_detail_home();
                    }
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::End) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    if picker.uses_navigable_popup() {
                        let (viewport, width) = navigable_popup_detail_metrics(terminal);
                        picker.scroll_detail_end();
                        picker.clamp_detail_scroll_for(width, viewport);
                    }
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.complete_filter();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.pop_filter_char();
                }
                Ok(true)
            }
            (KeyModifiers::CONTROL, KeyCode::Char('p')) if self.model_picker_is_open() => {
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Char(' ')) if self.picker_space_confirms_selection() => {
                self.submit_picker_selection_during_turn()?;
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let super::ComposerMode::Picker(picker) = &mut self.composer {
                    picker.push_filter_char(ch);
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.submit_picker_selection_during_turn()?;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.handle_picker_escape(/*running*/ true)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }
}
