use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{
    event::ToolCompletion,
    model::{Message, ToolResult},
};

/// Commits tool output for this run, independently of how tools are scheduled.
/// Restored dangling calls are deliberately not included in the pairing barrier.
#[derive(Default)]
pub(super) struct PendingToolOutputs {
    calls: BTreeSet<String>,
    finished: VecDeque<ToolResult>,
    images: BTreeMap<String, Message>,
    ready_images: Vec<Message>,
}

impl PendingToolOutputs {
    pub(super) fn register_calls<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) {
        self.calls.extend(ids.into_iter().map(str::to_owned));
    }

    pub(super) fn record_completion(
        &mut self,
        name: &str,
        id: &str,
        completion: &ToolCompletion,
    ) -> ToolResult {
        if let ToolCompletion::Success(output) = completion {
            if let Some(message) =
                Message::tool_image_supplement(name, id, output.images().to_vec())
            {
                self.images.insert(id.to_owned(), message);
            }
        }
        ToolResult {
            id: id.to_owned(),
            ok: matches!(completion, ToolCompletion::Success(_)),
            content: match completion {
                ToolCompletion::Success(output) => output.content().to_owned(),
                ToolCompletion::Failure(failure) => failure.message().to_owned(),
                ToolCompletion::Unavailable => format!("tool '{name}' is unavailable"),
            },
        }
    }

    pub(super) fn park_completion(&mut self, name: &str, id: &str, completion: &ToolCompletion) {
        let result = self.record_completion(name, id, completion);
        self.park_finished(result);
    }

    /// Park detached results before cancellable event publication.
    pub(super) fn park_finished(&mut self, result: ToolResult) {
        self.finished.push_back(result);
    }

    /// Keep supplements in result-commit order, behind every accepted call's result.
    pub(super) fn commit_results(
        &mut self,
        results: impl IntoIterator<Item = ToolResult>,
        history: &mut Vec<Message>,
    ) {
        for result in results {
            self.calls.remove(&result.id);
            if let Some(message) = self.images.remove(&result.id) {
                self.ready_images.push(message);
            }
            history.push(Message::ToolResult(result));
        }
        if self.calls.is_empty() {
            history.append(&mut self.ready_images);
        }
    }

    pub(super) fn drain_finished(&mut self, history: &mut Vec<Message>) -> usize {
        let results = std::mem::take(&mut self.finished);
        let count = results.len();
        self.commit_results(results, history);
        count
    }

    /// Terminal paths preserve committed history but never deliver more images.
    pub(super) fn discard_images(&mut self) {
        self.images.clear();
        self.ready_images.clear();
    }
}
