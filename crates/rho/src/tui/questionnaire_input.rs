use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use super::{
    questionnaire::{QuestionnaireComposer, QuestionnaireEnterAction},
    questionnaire_notice_text, App, ComposerMode, Entry, QuestionAnswerRequest,
};

impl App {
    pub(super) fn handle_questionnaire_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::Questionnaire(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                        questionnaire.clear_active_answer();
                    }
                    self.status = "answer cleared; press ctrl-c again to cancel".into();
                    self.ctrl_c_streak = 1;
                } else {
                    self.cancel_questionnaire_answer();
                }
                self.paste_burst.clear();
                Ok(true)
            }
            (KeyModifiers::ALT, KeyCode::Up) | (_, KeyCode::BackTab) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_to_previous_field();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::ALT, KeyCode::Down) | (_, KeyCode::Tab) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_to_next_field();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::ALT, KeyCode::Backspace) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.delete_previous_word();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let action = match &mut self.composer {
                    ComposerMode::Questionnaire(questionnaire) => {
                        questionnaire.confirm_active_question()
                    }
                    _ => unreachable!("questionnaire mode checked before key handling"),
                };
                match action {
                    QuestionnaireEnterAction::Advance => {}
                    QuestionnaireEnterAction::Submit => self.submit_questionnaire_answer()?,
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.cancel_questionnaire_answer();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Up) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_up();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Down) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_down();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Backspace) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.backspace();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Delete) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.delete();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::ALT, KeyCode::Left) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_text_cursor_previous_word();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::ALT, KeyCode::Right) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_text_cursor_next_word();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Left) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_cursor_left();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Right) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_cursor_right();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Home) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_cursor_home();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::End) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.move_cursor_end();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::CONTROL, KeyCode::Char('j')) | (KeyModifiers::ALT, KeyCode::Enter) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.insert_char('\n');
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.insert_char('\n');
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Char(' ')) => {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    if questionnaire.active_text_entry_active() {
                        questionnaire.insert_char(' ');
                    } else {
                        questionnaire.toggle_active_choice();
                    }
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let ComposerMode::Questionnaire(questionnaire) = &mut self.composer {
                    questionnaire.insert_char(ch);
                }
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
        }
    }

    fn submit_questionnaire_answer(&mut self) -> anyhow::Result<()> {
        if let Some(display) = self.prepare_questionnaire_answer()? {
            self.insert_entry(&Entry::User(display));
        }
        Ok(())
    }

    fn prepare_questionnaire_answer(&mut self) -> anyhow::Result<Option<String>> {
        let ComposerMode::Questionnaire(mut questionnaire) =
            std::mem::replace(&mut self.composer, ComposerMode::Input)
        else {
            return Ok(None);
        };
        match questionnaire.submit() {
            Ok(submitted) => {
                let display = submitted.display;
                self.input.clear();
                self.paste_segments.clear();
                self.input_cursor = 0;
                self.command_palette_dismissed = false;
                self.clamp_command_selection();
                self.status = "answers submitted".into();
                Ok(Some(display))
            }
            Err(error) => {
                self.composer = ComposerMode::Questionnaire(questionnaire);
                self.status = error;
                Ok(None)
            }
        }
    }

    fn cancel_questionnaire_answer(&mut self) {
        let ComposerMode::Questionnaire(mut questionnaire) =
            std::mem::replace(&mut self.composer, ComposerMode::Input)
        else {
            return;
        };
        questionnaire.cancel_by_user();
        self.ctrl_c_streak = 0;
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.status = "answer cancelled".into();
    }

    pub(super) fn open_questionnaire(
        &mut self,
        request: QuestionAnswerRequest,
        _terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        self.finish_streams();
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.insert_entry(&Entry::Notice(questionnaire_notice_text(&request.request)));
        self.composer = ComposerMode::Questionnaire(QuestionnaireComposer::new(
            request.request,
            request.response,
        ));
        self.status = "waiting for your answers".into();
        Ok(())
    }
}
