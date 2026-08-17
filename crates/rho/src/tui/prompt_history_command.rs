use super::{
    config_picker, App, ComposerMode, ConfigNumberInput, ConfigNumberKey, Entry, InlineChoice,
    InlineChoiceModal, InlineChoiceOption, InlineChoicePending, UiPicker,
};

const CONFIRM_VALUE: &str = "confirm";
const CANCEL_VALUE: &str = "cancel";

struct PromptHistoryChoice {
    title: String,
    description: String,
    confirm_label: &'static str,
    confirm_detail: &'static str,
    cancel_label: &'static str,
    pending: InlineChoicePending,
    status: &'static str,
}

impl App {
    pub(super) fn open_prompt_history_limit_editor(&mut self) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        self.input_ui
            .set_composer(ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                ConfigNumberKey::PromptHistoryLimit,
                config.prompt_history_limit,
            )));
        self.set_status("edit prompt history limit");
        Ok(())
    }

    pub(super) fn propose_prompt_history_limit(&mut self, new_limit: usize) -> anyhow::Result<()> {
        let stored = self.stored_prompt_history_count();
        if new_limit > 0 && stored > new_limit {
            let dropped = stored - new_limit;
            self.prompt_prompt_history_choice(PromptHistoryChoice {
                title: format!("Keep only {new_limit} prompts?"),
                description: format!(
                    "This permanently deletes {dropped} older saved prompt{}.",
                    if dropped == 1 { "" } else { "s" }
                ),
                confirm_label: "Delete older prompts",
                confirm_detail: "This cannot be undone",
                cancel_label: "Keep the current saved history",
                pending: InlineChoicePending::PromptHistoryLimit { new_limit },
                status: "confirm prompt history limit",
            })
        } else if new_limit == 0 && stored > 0 {
            self.prompt_prompt_history_choice(PromptHistoryChoice {
                title: "Stop saving prompt history?".into(),
                description:
                    "New prompts will not be stored. Existing history is kept until you clear it."
                        .into(),
                confirm_label: "Disable saving",
                confirm_detail: "Existing prompts stay on disk",
                cancel_label: "Keep saving prompts",
                pending: InlineChoicePending::PromptHistoryLimit { new_limit },
                status: "confirm disable prompt history",
            })
        } else {
            self.apply_prompt_history_limit(new_limit)
        }
    }

    pub(super) fn prompt_clear_prompt_history(&mut self) -> anyhow::Result<()> {
        let stored = self.stored_prompt_history_count();
        if stored == 0 && self.input_ui.history().is_empty() {
            self.set_status("prompt history is already empty");
            return Ok(());
        }
        self.prompt_prompt_history_choice(PromptHistoryChoice {
            title: "Clear prompt history?".into(),
            description: "This permanently deletes every saved composer prompt, including this session's up-arrow recall.".into(),
            confirm_label: "Clear history",
            confirm_detail: "This cannot be undone",
            cancel_label: "Keep the saved prompts",
            pending: InlineChoicePending::ClearPromptHistory,
            status: "confirm clear prompt history",
        })
    }

    fn prompt_prompt_history_choice(&mut self, prompt: PromptHistoryChoice) -> anyhow::Result<()> {
        let choice = InlineChoice::new(
            prompt.title,
            prompt.description,
            vec![
                InlineChoiceOption::available(
                    CONFIRM_VALUE,
                    'y',
                    prompt.confirm_label,
                    prompt.confirm_detail,
                ),
                InlineChoiceOption::available(
                    CANCEL_VALUE,
                    'n',
                    prompt.cancel_label,
                    "Leave history as it is",
                )
                .with_alternate_shortcut('c'),
            ],
        )?;
        let parent_picker = take_config_parent_picker(self);
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: prompt.pending,
                parent_picker,
            }));
        self.set_status(prompt.status);
        Ok(())
    }

    pub(super) fn submit_prompt_history_limit_choice(
        &mut self,
        value: &str,
        new_limit: usize,
    ) -> anyhow::Result<()> {
        if value != CONFIRM_VALUE {
            return self.restore_prompt_history_config(config_picker::PROMPT_HISTORY_LIMIT_VALUE);
        }
        self.apply_prompt_history_limit(new_limit)
    }

    pub(super) fn submit_clear_prompt_history_choice(&mut self, value: &str) -> anyhow::Result<()> {
        if value != CONFIRM_VALUE {
            return self.restore_prompt_history_config(config_picker::CLEAR_PROMPT_HISTORY_VALUE);
        }
        self.clear_prompt_history()
    }

    pub(super) fn restore_prompt_history_config(&mut self, selected: &str) -> anyhow::Result<()> {
        self.open_main_config_picker_selected(selected)
    }

    fn apply_prompt_history_limit(&mut self, new_limit: usize) -> anyhow::Result<()> {
        if let Err(error) = self.info.services.config_repository.update(|config| {
            config.prompt_history_limit = new_limit;
        }) {
            self.insert_entry(&Entry::Error(format!(
                "could not save prompt history limit: {error}"
            )));
            self.restore_prompt_history_config(config_picker::PROMPT_HISTORY_LIMIT_VALUE)?;
            self.set_status("config save failed");
            return Ok(());
        }
        self.prompt_history_limit = new_limit;
        if new_limit > 0 {
            if let Err(error) =
                self.with_prompt_history_store(|store| store.enforce_limit(new_limit))
            {
                self.insert_entry(&Entry::Error(format!(
                    "could not trim prompt history: {error}"
                )));
            }
            self.input_ui.truncate_history_to_newest(new_limit);
        }
        self.restore_prompt_history_config(config_picker::PROMPT_HISTORY_LIMIT_VALUE)?;
        self.set_status(if new_limit == 0 {
            "prompt history saving disabled".into()
        } else {
            format!("prompt history limit set to {new_limit}")
        });
        Ok(())
    }

    fn clear_prompt_history(&mut self) -> anyhow::Result<()> {
        self.input_ui.clear_history();
        let _ = self
            .prompt_history_tx
            .send(super::prompt_history_persistence::PromptHistoryOp::Clear);
        if let Err(error) = self.with_prompt_history_store(|store| store.clear()) {
            self.insert_entry(&Entry::Error(format!(
                "could not clear prompt history: {error}"
            )));
            self.restore_prompt_history_config(config_picker::CLEAR_PROMPT_HISTORY_VALUE)?;
            self.set_status("clear prompt history failed");
            return Ok(());
        }
        self.restore_prompt_history_config(config_picker::CLEAR_PROMPT_HISTORY_VALUE)?;
        self.set_status("prompt history cleared");
        Ok(())
    }

    fn stored_prompt_history_count(&self) -> usize {
        self.with_prompt_history_store(|store| store.count())
            .unwrap_or(0)
    }

    fn with_prompt_history_store<T>(
        &self,
        op: impl FnOnce(
            &crate::prompt_history::PromptHistoryStore,
        ) -> Result<T, crate::prompt_history::PromptHistoryError>,
    ) -> Result<T, crate::prompt_history::PromptHistoryError> {
        let store = match &self.prompt_history_store_path {
            Some(path) => crate::prompt_history::PromptHistoryStore::open_path(path),
            None => crate::prompt_history::PromptHistoryStore::at_default_path(),
        }?;
        op(&store)
    }

    #[cfg(test)]
    pub(super) fn take_prompt_history_rx(
        &mut self,
    ) -> Option<
        tokio::sync::mpsc::UnboundedReceiver<super::prompt_history_persistence::PromptHistoryOp>,
    > {
        self.prompt_history_rx.take()
    }
}

fn take_config_parent_picker(app: &mut App) -> Option<Box<UiPicker>> {
    match app.input_ui.take_composer() {
        ComposerMode::Picker(picker) => Some(Box::new(picker)),
        composer => {
            app.input_ui.set_composer(composer);
            None
        }
    }
}

#[cfg(test)]
#[path = "prompt_history_command_tests.rs"]
mod tests;
