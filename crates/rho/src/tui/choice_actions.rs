use ratatui::DefaultTerminal;

use super::{App, ComposerMode, InlineChoiceKeyOutcome, InlineChoicePending, InteractiveRuntime};

impl App {
    pub(super) async fn handle_inline_choice_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let outcome = match &mut self.composer {
            ComposerMode::InlineChoice(modal) => modal.choice.handle_key(key),
            _ => return Ok(false),
        };

        match outcome {
            InlineChoiceKeyOutcome::Selected(value) => {
                let ComposerMode::InlineChoice(modal) =
                    std::mem::replace(&mut self.composer, ComposerMode::Input)
                else {
                    unreachable!("inline choice checked above");
                };
                match modal.pending {
                    InlineChoicePending::CredentialStore { next } => {
                        self.submit_credential_store_choice(modal.choice, next, terminal, agent)
                            .await?;
                    }
                    InlineChoicePending::ContextHandoff(pending) => {
                        self.resolve_context_handoff(Some(&value), *pending, terminal, agent)
                            .await?;
                    }
                }
            }
            InlineChoiceKeyOutcome::Cancelled => {
                let ComposerMode::InlineChoice(modal) =
                    std::mem::replace(&mut self.composer, ComposerMode::Input)
                else {
                    unreachable!("inline choice checked above");
                };
                match modal.pending {
                    InlineChoicePending::CredentialStore { .. } => {
                        self.status = if self.is_ui_busy() {
                            "running"
                        } else {
                            "ready"
                        }
                        .into();
                    }
                    InlineChoicePending::ContextHandoff(pending) => {
                        self.resolve_context_handoff(None, *pending, terminal, agent)
                            .await?;
                    }
                }
            }
            InlineChoiceKeyOutcome::Handled => {}
        }
        self.paste_burst.clear();
        self.ctrl_c_streak = 0;
        Ok(true)
    }
}
