//! Shared composer prompt history: one owner, one writer thread.

use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

use futures_util::FutureExt;

use crate::prompt_history::{PromptHistoryError, PromptHistoryLoadHandle, PromptHistoryStore};

use super::{
    config_picker, App, ComposerMode, ConfigNumberInput, ConfigNumberKey, Entry, InlineChoice,
    InlineChoiceModal, InlineChoiceOption, InlineChoicePending, UiPicker,
};

const MAX_PERSISTED_PROMPT_BYTES: usize = 10 * 1024;
const CONFIRM_VALUE: &str = "confirm";
const CANCEL_VALUE: &str = "cancel";

enum StoreOp {
    Append {
        text: String,
        max_entries: usize,
    },
    Count(Sender<StoreReply>),
    Clear(Sender<StoreReply>),
    Enforce {
        max_entries: usize,
        reply: Sender<StoreReply>,
    },
    SetPath(Option<PathBuf>),
    Flush(Sender<()>),
}

enum StoreReply {
    Count(Result<usize, PromptHistoryError>),
    Done(Result<(), PromptHistoryError>),
}

enum FollowUp {
    ProposeLimit { new_limit: usize },
    FinishLimit { new_limit: usize },
    PromptClear,
    FinishClear,
}

pub(in crate::tui) struct PromptHistory {
    store_path: Option<PathBuf>,
    limit: usize,
    pending_load: Option<PromptHistoryLoadHandle>,
    ring_invalidated: bool,
    tx: Sender<StoreOp>,
    pending_reply: Option<Receiver<StoreReply>>,
    follow_up: Option<FollowUp>,
}

impl PromptHistory {
    pub(in crate::tui) fn new(limit: usize, pending_load: Option<PromptHistoryLoadHandle>) -> Self {
        Self {
            store_path: None,
            limit,
            pending_load,
            ring_invalidated: false,
            tx: spawn_writer(None),
            pending_reply: None,
            follow_up: None,
        }
    }

    pub(in crate::tui) fn limit(&self) -> usize {
        self.limit
    }

    pub(in crate::tui) fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }

    pub(in crate::tui) fn load_pending(&self) -> bool {
        self.pending_load.is_some() || self.pending_reply.is_some()
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
        if self
            .tx
            .send(StoreOp::Append {
                text: text.to_string(),
                max_entries: self.limit,
            })
            .is_err()
        {
            tracing::warn!("prompt history writer is gone");
        }
    }

    fn request_count(&mut self) -> bool {
        self.request_reply(StoreOp::Count)
    }

    fn request_clear(&mut self) -> bool {
        self.ring_invalidated = true;
        self.request_reply(StoreOp::Clear)
    }

    fn request_enforce(&mut self, max_entries: usize) -> bool {
        self.request_reply(|reply| StoreOp::Enforce { max_entries, reply })
    }

    fn request_reply(&mut self, op: impl FnOnce(Sender<StoreReply>) -> StoreOp) -> bool {
        if self.pending_reply.is_some() {
            return false;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        if self.tx.send(op(reply_tx)).is_err() {
            tracing::warn!("prompt history writer is gone");
            return false;
        }
        self.pending_reply = Some(reply_rx);
        true
    }

    fn take_ready_reply(&mut self) -> Option<StoreReply> {
        let rx = self.pending_reply.as_ref()?;
        match rx.try_recv() {
            Ok(reply) => {
                self.pending_reply = None;
                Some(reply)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_reply = None;
                Some(StoreReply::Done(Err(PromptHistoryError::DataDirectory)))
            }
        }
    }

    fn take_finished_seed(&mut self) -> Option<Vec<String>> {
        let handle = self.pending_load.as_mut()?;
        if !handle.is_finished() {
            return None;
        }
        let handle = self.pending_load.take()?;
        match handle.now_or_never() {
            Some(Ok(Some((_store, tail)))) => self.seed_from_load(tail),
            _ => None,
        }
    }

    fn seed_from_load(&self, mut tail: Vec<String>) -> Option<Vec<String>> {
        if self.ring_invalidated {
            return None;
        }
        if self.limit > 0 && tail.len() > self.limit {
            tail = tail.split_off(tail.len() - self.limit);
        }
        Some(tail)
    }

    #[cfg(test)]
    pub(in crate::tui) fn set_store_path(&mut self, path: PathBuf) {
        self.store_path = Some(path.clone());
        let _ = self.tx.send(StoreOp::SetPath(Some(path)));
    }

    #[cfg(test)]
    pub(in crate::tui) fn store_path(&self) -> Option<&std::path::Path> {
        self.store_path.as_deref()
    }

    #[cfg(test)]
    pub(in crate::tui) fn flush(&mut self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(StoreOp::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }
}

fn spawn_writer(path: Option<PathBuf>) -> Sender<StoreOp> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("rho-prompt-history".into())
        .spawn(move || writer_loop(rx, path));
    tx
}

fn writer_loop(rx: Receiver<StoreOp>, path: Option<PathBuf>) {
    let mut state = WriterState { store: None, path };
    while let Ok(op) = rx.recv() {
        match op {
            StoreOp::Append { text, max_entries } => match state.ensure_create() {
                Ok(store) => {
                    if let Err(error) = store.append(&text, max_entries) {
                        tracing::warn!(%error, "failed to append prompt history");
                    }
                }
                Err(error) => tracing::warn!(%error, "failed to open prompt history"),
            },
            StoreOp::Count(reply) => {
                let result = state
                    .ensure_existing()
                    .and_then(|store| store.map(PromptHistoryStore::count).transpose())
                    .map(|count| count.unwrap_or(0));
                let _ = reply.send(StoreReply::Count(result));
            }
            StoreOp::Clear(reply) => {
                let result = state
                    .ensure_existing()
                    .and_then(|store| store.map(PromptHistoryStore::clear).transpose())
                    .map(|_| ());
                let _ = reply.send(StoreReply::Done(result));
            }
            StoreOp::Enforce { max_entries, reply } => {
                let result = state
                    .ensure_existing()
                    .and_then(|store| {
                        store
                            .map(|store| store.enforce_limit(max_entries))
                            .transpose()
                    })
                    .map(|_| ());
                let _ = reply.send(StoreReply::Done(result));
            }
            StoreOp::SetPath(path) => {
                state.store = None;
                state.path = path;
            }
            StoreOp::Flush(reply) => {
                let _ = reply.send(());
            }
        }
    }
}

struct WriterState {
    store: Option<PromptHistoryStore>,
    path: Option<PathBuf>,
}

impl WriterState {
    fn ensure_create(&mut self) -> Result<&PromptHistoryStore, PromptHistoryError> {
        if self.store.is_none() {
            self.store = Some(match &self.path {
                Some(path) => PromptHistoryStore::open_path(path),
                None => PromptHistoryStore::at_default_path(),
            }?);
        }
        Ok(self.store.as_ref().expect("store just inserted"))
    }

    fn ensure_existing(&mut self) -> Result<Option<&PromptHistoryStore>, PromptHistoryError> {
        if self.store.is_none() {
            self.store = match &self.path {
                Some(path) => PromptHistoryStore::open_path_if_exists(path)?,
                None => PromptHistoryStore::at_default_path_if_exists()?,
            };
        }
        Ok(self.store.as_ref())
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
        let mut changed = false;
        if let Some(tail) = self.prompt_history.take_finished_seed() {
            let seeded = !tail.is_empty();
            self.input_ui.seed_history_front(tail);
            changed |= seeded;
        }
        if let Some(reply) = self.prompt_history.take_ready_reply() {
            changed |= self.handle_prompt_history_reply(reply);
        }
        changed
    }

    #[cfg(test)]
    pub(super) fn settle_prompt_history(&mut self) {
        for _ in 0..8 {
            self.prompt_history.flush();
            if !self.poll_prompt_history() && self.prompt_history.pending_reply.is_none() {
                break;
            }
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
        if !self.prompt_history.request_count() {
            self.set_status("prompt history is busy");
            return Ok(());
        }
        self.prompt_history.follow_up = Some(FollowUp::ProposeLimit { new_limit });
        self.set_status("reading prompt history");
        Ok(())
    }

    pub(super) fn prompt_clear_prompt_history(&mut self) -> anyhow::Result<()> {
        if !self.prompt_history.request_count() {
            self.set_status("prompt history is busy");
            return Ok(());
        }
        self.prompt_history.follow_up = Some(FollowUp::PromptClear);
        self.set_status("reading prompt history");
        Ok(())
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
            self.input_ui.truncate_history_to_newest(new_limit);
            if self.prompt_history.request_enforce(new_limit) {
                self.prompt_history.follow_up = Some(FollowUp::FinishLimit { new_limit });
                self.set_status("updating prompt history");
                return Ok(());
            }
        }
        self.finish_prompt_history_limit(new_limit)
    }

    fn finish_prompt_history_limit(&mut self, new_limit: usize) -> anyhow::Result<()> {
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
        if !self.prompt_history.request_clear() {
            self.insert_entry(&Entry::Error(
                "could not clear prompt history: writer is busy".into(),
            ));
            self.open_main_config_picker_selected(config_picker::CLEAR_PROMPT_HISTORY_VALUE)?;
            self.set_status("clear prompt history failed");
            return Ok(());
        }
        self.prompt_history.follow_up = Some(FollowUp::FinishClear);
        self.set_status("clearing prompt history");
        Ok(())
    }

    fn handle_prompt_history_reply(&mut self, reply: StoreReply) -> bool {
        let follow_up = self.prompt_history.follow_up.take();
        let result = match (follow_up, reply) {
            (Some(FollowUp::ProposeLimit { new_limit }), StoreReply::Count(result)) => {
                self.finish_propose_prompt_history_limit(new_limit, result)
            }
            (Some(FollowUp::PromptClear), StoreReply::Count(result)) => {
                self.finish_prompt_clear_prompt_history(result)
            }
            (Some(FollowUp::FinishLimit { new_limit }), StoreReply::Done(result)) => {
                if let Err(error) = result {
                    self.insert_entry(&Entry::Error(format!(
                        "could not trim prompt history: {error}"
                    )));
                }
                self.finish_prompt_history_limit(new_limit)
            }
            (Some(FollowUp::FinishClear), StoreReply::Done(result)) => match result {
                Ok(()) => {
                    let done = self.open_main_config_picker_selected(
                        config_picker::CLEAR_PROMPT_HISTORY_VALUE,
                    );
                    self.set_status("prompt history cleared");
                    done
                }
                Err(error) => {
                    self.insert_entry(&Entry::Error(format!(
                        "could not clear prompt history: {error}"
                    )));
                    let done = self.open_main_config_picker_selected(
                        config_picker::CLEAR_PROMPT_HISTORY_VALUE,
                    );
                    self.set_status("clear prompt history failed");
                    done
                }
            },
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.insert_entry(&Entry::Error(error.to_string()));
        }
        true
    }

    fn finish_propose_prompt_history_limit(
        &mut self,
        new_limit: usize,
        stored: Result<usize, PromptHistoryError>,
    ) -> anyhow::Result<()> {
        let stored = match stored {
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

    fn finish_prompt_clear_prompt_history(
        &mut self,
        stored: Result<usize, PromptHistoryError>,
    ) -> anyhow::Result<()> {
        let stored = match stored {
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
