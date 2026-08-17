//! Feature policy for persisting composer history to the shared SQLite store.

use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::FutureExt;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::prompt_history::PromptHistoryStore;

use super::App;

pub(in crate::tui) const MAX_PERSISTED_PROMPT_BYTES: usize = 10 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) enum PromptHistoryOp {
    Append(String),
    Clear,
}

pub(in crate::tui) fn eligible_for_persistence(text: &str) -> bool {
    text.len() <= MAX_PERSISTED_PROMPT_BYTES
}

pub(in crate::tui) fn spawn_prompt_history_writer(
    store: PromptHistoryStore,
    limit: usize,
    mut rx: UnboundedReceiver<PromptHistoryOp>,
) {
    tokio::spawn(async move {
        let mut warned_append = false;
        let mut warned_clear = false;
        while let Some(op) = rx.recv().await {
            let store = store.clone();
            let kind = op.clone();
            let result = tokio::task::spawn_blocking(move || match op {
                PromptHistoryOp::Append(text) => store.append(&text, now_ms(), limit),
                PromptHistoryOp::Clear => store.clear(),
            })
            .await;
            match (kind, result) {
                (_, Ok(Ok(()))) => {}
                (PromptHistoryOp::Append(_), error) if !warned_append => {
                    warned_append = true;
                    tracing::warn!(?error, "failed to append prompt history");
                }
                (PromptHistoryOp::Clear, error) if !warned_clear => {
                    warned_clear = true;
                    tracing::warn!(?error, "failed to clear prompt history");
                }
                _ => {}
            }
        }
    });
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
    )
    .unwrap_or(i64::MAX)
}

impl App {
    pub(super) fn poll_prompt_history(&mut self) -> bool {
        let Some(handle) = self.pending_prompt_history.as_mut() else {
            return false;
        };
        if !handle.is_finished() {
            return false;
        }
        let Some(handle) = self.pending_prompt_history.take() else {
            return false;
        };
        match handle.now_or_never() {
            Some(Ok(Some((store, tail, limit)))) => {
                let seeded = !tail.is_empty();
                self.input_ui.seed_history_front(tail);
                if let Some(rx) = self.prompt_history_rx.take() {
                    spawn_prompt_history_writer(store, limit, rx);
                }
                seeded
            }
            _ => {
                self.prompt_history_rx = None;
                false
            }
        }
    }
}
