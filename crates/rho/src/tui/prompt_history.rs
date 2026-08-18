//! Shared composer prompt history: one store handle, one mutation path.

use std::path::PathBuf;

use futures_util::FutureExt;

use crate::prompt_history::{PromptHistoryError, PromptHistoryLoadHandle, PromptHistoryStore};

use super::{
    config_picker, App, ComposerMode, ConfigNumberInput, ConfigNumberKey, Entry, InlineChoice,
    InlineChoiceModal, InlineChoiceOption, InlineChoicePending, UiPicker,
};

const MAX_PERSISTED_PROMPT_BYTES: usize = 10 * 1024;
const CONFIRM_VALUE: &str = "confirm";
const CANCEL_VALUE: &str = "cancel";

pub(in crate::tui) struct PromptHistory {
    store: Option<PromptHistoryStore>,
    store_path: Option<PathBuf>,
    limit: usize,
    pending_load: Option<PromptHistoryLoadHandle>,
    ring_invalidated: bool,
}

impl PromptHistory {
    pub(in crate::tui) fn new(limit: usize, pending_load: Option<PromptHistoryLoadHandle>) -> Self {
        Self {
            store: None,
            store_path: None,
            limit,
            pending_load,
            ring_invalidated: false,
        }
    }

    pub(in crate::tui) fn limit(&self) -> usize {
        self.limit
    }

    pub(in crate::tui) fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }

    pub(in crate::tui) fn load_pending(&self) -> bool {
        self.pending_load.is_some()
    }

    pub(in crate::tui) fn load_finished(&self) -> bool {
        self.pending_load
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
    }

    pub(in crate::tui) fn push(&mut self, text: &str) {
        if self.limit == 0 || text.len() > MAX_PERSISTED_PROMPT_BYTES {
            return;
        }
        let limit = self.limit;
        if let Err(error) = self
            .ensure_store()
            .and_then(|store| store.append(text, limit))
        {
            tracing::warn!(%error, "failed to append prompt history");
        }
    }

    pub(in crate::tui) fn clear(&mut self) -> Result<(), PromptHistoryError> {
        self.ring_invalidated = true;
        match self.store_if_present()? {
            Some(store) => store.clear(),
            None => Ok(()),
        }
    }

    pub(in crate::tui) fn count(&mut self) -> Result<usize, PromptHistoryError> {
        match self.store_if_present()? {
            Some(store) => store.count(),
            None => Ok(0),
        }
    }

    pub(in crate::tui) fn enforce_limit(
        &mut self,
        max_entries: usize,
    ) -> Result<(), PromptHistoryError> {
        self.ring_invalidated = true;
        match self.store_if_present()? {
            Some(store) => store.enforce_limit(max_entries),
            None => Ok(()),
        }
    }

    fn take_finished_seed(&mut self) -> Option<Vec<String>> {
        let handle = self.pending_load.as_mut()?;
        if !handle.is_finished() {
            return None;
        }
        let handle = self.pending_load.take()?;
        match handle.now_or_never() {
            Some(Ok(Some((store, tail)))) => {
                if self.store.is_none() {
                    self.store = Some(store);
                }
                self.seed_from_load(tail)
            }
            _ => None,
        }
    }

    fn seed_from_load(&self, tail: Vec<String>) -> Option<Vec<String>> {
        if self.ring_invalidated {
            None
        } else {
            Some(tail)
        }
    }

    fn ensure_store(&mut self) -> Result<&PromptHistoryStore, PromptHistoryError> {
        if self.store.is_none() {
            self.store = Some(self.open_store()?);
        }
        Ok(self.store.as_ref().expect("store just inserted"))
    }

    fn store_if_present(&mut self) -> Result<Option<&PromptHistoryStore>, PromptHistoryError> {
        if self.store.is_none() {
            self.store = self.open_existing_store()?;
        }
        Ok(self.store.as_ref())
    }

    fn open_store(&self) -> Result<PromptHistoryStore, PromptHistoryError> {
        match &self.store_path {
            Some(path) => PromptHistoryStore::open_path(path),
            None => PromptHistoryStore::at_default_path(),
        }
    }

    fn open_existing_store(&self) -> Result<Option<PromptHistoryStore>, PromptHistoryError> {
        match &self.store_path {
            Some(path) => PromptHistoryStore::open_path_if_exists(path),
            None => PromptHistoryStore::at_default_path_if_exists(),
        }
    }

    #[cfg(test)]
    pub(in crate::tui) fn set_store_path(&mut self, path: PathBuf) {
        self.store = None;
        self.store_path = Some(path);
    }

    #[cfg(test)]
    pub(in crate::tui) fn store_path(&self) -> Option<&std::path::Path> {
        self.store_path.as_deref()
    }
}

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
    pub(super) fn poll_prompt_history(&mut self) -> bool {
        match self.prompt_history.take_finished_seed() {
            Some(tail) => {
                let seeded = !tail.is_empty();
                self.input_ui.seed_history_front(tail);
                seeded
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub(super) fn apply_loaded_prompt_history_seed(&mut self, tail: Vec<String>) -> bool {
        match self.prompt_history.seed_from_load(tail) {
            Some(tail) => {
                let seeded = !tail.is_empty();
                self.input_ui.seed_history_front(tail);
                seeded
            }
            None => false,
        }
    }

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
        let stored = match self.prompt_history.count() {
            Ok(count) => count,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not read prompt history: {error}"
                )));
                self.open_main_config_picker_selected(config_picker::PROMPT_HISTORY_LIMIT_VALUE)?;
                self.set_status("prompt history unavailable");
                return Ok(());
            }
        };
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
        let stored = match self.prompt_history.count() {
            Ok(count) => count,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not read prompt history: {error}"
                )));
                self.open_main_config_picker_selected(config_picker::CLEAR_PROMPT_HISTORY_VALUE)?;
                self.set_status("prompt history unavailable");
                return Ok(());
            }
        };
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
            return self
                .open_main_config_picker_selected(config_picker::PROMPT_HISTORY_LIMIT_VALUE);
        }
        self.apply_prompt_history_limit(new_limit)
    }

    pub(super) fn submit_clear_prompt_history_choice(&mut self, value: &str) -> anyhow::Result<()> {
        if value != CONFIRM_VALUE {
            return self
                .open_main_config_picker_selected(config_picker::CLEAR_PROMPT_HISTORY_VALUE);
        }
        self.clear_prompt_history()
    }

    fn apply_prompt_history_limit(&mut self, new_limit: usize) -> anyhow::Result<()> {
        if let Err(error) = self.info.services.config_repository.update(|config| {
            config.prompt_history_limit = new_limit;
        }) {
            self.insert_entry(&Entry::Error(format!(
                "could not save prompt history limit: {error}"
            )));
            self.open_main_config_picker_selected(config_picker::PROMPT_HISTORY_LIMIT_VALUE)?;
            self.set_status("config save failed");
            return Ok(());
        }
        self.prompt_history.set_limit(new_limit);
        if new_limit > 0 {
            if let Err(error) = self.prompt_history.enforce_limit(new_limit) {
                self.insert_entry(&Entry::Error(format!(
                    "could not trim prompt history: {error}"
                )));
            }
            self.input_ui.truncate_history_to_newest(new_limit);
        }
        self.open_main_config_picker_selected(config_picker::PROMPT_HISTORY_LIMIT_VALUE)?;
        self.set_status(if new_limit == 0 {
            "prompt history saving disabled".into()
        } else {
            format!("prompt history limit set to {new_limit}")
        });
        Ok(())
    }

    fn clear_prompt_history(&mut self) -> anyhow::Result<()> {
        self.input_ui.clear_history();
        if let Err(error) = self.prompt_history.clear() {
            self.insert_entry(&Entry::Error(format!(
                "could not clear prompt history: {error}"
            )));
            self.open_main_config_picker_selected(config_picker::CLEAR_PROMPT_HISTORY_VALUE)?;
            self.set_status("clear prompt history failed");
            return Ok(());
        }
        self.open_main_config_picker_selected(config_picker::CLEAR_PROMPT_HISTORY_VALUE)?;
        self.set_status("prompt history cleared");
        Ok(())
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
#[path = "prompt_history_tests.rs"]
mod tests;
