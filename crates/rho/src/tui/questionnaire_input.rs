use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    questionnaire::{QuestionnaireComposer, QuestionnaireEnterAction},
    questionnaire_notice_text, App, ComposerMode, Entry, HerdrUserWait, QuestionAnswerRequest,
};

impl App {
    pub(super) fn handle_questionnaire_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), ComposerMode::Questionnaire(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    if let Some(questionnaire) = self.questionnaire_mut() {
                        questionnaire.clear_active_answer();
                    }
                    self.status = "answer cleared; press ctrl-c again to cancel".into();
                    self.ctrl_c_streak = 1;
                } else {
                    self.cancel_questionnaire_answer();
                }
            }
            (KeyModifiers::ALT, KeyCode::Up) | (_, KeyCode::BackTab) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_to_previous_field();
                }
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Down) | (_, KeyCode::Tab) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_to_next_field();
                }
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Backspace) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.delete_previous_word();
                }
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let action = match self.questionnaire_mut() {
                    Some(questionnaire) => questionnaire.confirm_active_question(),
                    None => unreachable!("questionnaire mode checked before key handling"),
                };
                match action {
                    QuestionnaireEnterAction::Advance => {}
                    QuestionnaireEnterAction::Submit => self.submit_questionnaire_answer()?,
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Esc) => {
                self.cancel_questionnaire_answer();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Up) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_up();
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Down) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_down();
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Backspace) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.backspace();
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Delete) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.delete();
                }
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Left) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_text_cursor_previous_word();
                }
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Right) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_text_cursor_next_word();
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Left) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_cursor_left();
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Right) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_cursor_right();
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Home) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_cursor_home();
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::End) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.move_cursor_end();
                }
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('j')) | (KeyModifiers::ALT, KeyCode::Enter) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.insert_char('\n');
                }
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.insert_char('\n');
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Char(' ')) => {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    if questionnaire.active_text_entry_active() {
                        questionnaire.insert_char(' ');
                    } else {
                        questionnaire.toggle_active_choice();
                    }
                }
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(questionnaire) = self.questionnaire_mut() {
                    questionnaire.insert_char(ch);
                }
                self.ctrl_c_streak = 0;
            }
            _ => {
                self.ctrl_c_streak = 0;
            }
        }
        self.input_ui.clear_paste_burst();
        Ok(true)
    }

    fn questionnaire_mut(&mut self) -> Option<&mut QuestionnaireComposer> {
        match self.input_ui.composer_mut() {
            ComposerMode::Questionnaire(questionnaire) => Some(questionnaire),
            _ => None,
        }
    }

    fn submit_questionnaire_answer(&mut self) -> anyhow::Result<()> {
        if let Some(display) = self.prepare_questionnaire_answer()? {
            self.insert_entry(&Entry::User(display));
        }
        Ok(())
    }

    fn prepare_questionnaire_answer(&mut self) -> anyhow::Result<Option<String>> {
        let ComposerMode::Questionnaire(mut questionnaire) = self.input_ui.take_composer() else {
            return Ok(None);
        };
        match questionnaire.submit() {
            Ok(submitted) => {
                let display = submitted.display;
                self.clear_submitted_input();
                self.status = "answers submitted".into();
                Ok(Some(display))
            }
            Err(error) => {
                self.input_ui
                    .set_composer(ComposerMode::Questionnaire(questionnaire));
                self.status = error;
                Ok(None)
            }
        }
    }

    fn cancel_questionnaire_answer(&mut self) {
        let ComposerMode::Questionnaire(mut questionnaire) = self.input_ui.take_composer() else {
            return;
        };
        questionnaire.cancel_by_user();
        self.ctrl_c_streak = 0;
        self.clear_submitted_input();
        self.status = "answer cancelled".into();
    }

    pub(super) async fn open_questionnaire(
        &mut self,
        request: QuestionAnswerRequest,
    ) -> std::io::Result<()> {
        self.finish_streams();
        self.clear_submitted_input();
        self.insert_entry(&Entry::Notice(questionnaire_notice_text(&request.request)));
        self.input_ui
            .set_composer(ComposerMode::Questionnaire(QuestionnaireComposer::new(
                request.request,
                request.response,
            )));
        self.status = HerdrUserWait::Questionnaire.message().into();
        self.report_herdr_waiting_for_user(HerdrUserWait::Questionnaire)
            .await;
        Ok(())
    }
}
