use crate::{
    event::ToolCompletion,
    model::{Message, ToolResult},
};

/// One completion owns its textual result and optional image supplement together.
/// Schedulers may park it, but cannot commit its image without its result.
pub(super) struct CompletedToolOutput {
    result: ToolResult,
    supplement: Option<Message>,
}

impl CompletedToolOutput {
    pub(super) fn new(name: &str, id: &str, completion: &ToolCompletion) -> Self {
        Self {
            result: ToolResult {
                id: id.to_owned(),
                ok: matches!(completion, ToolCompletion::Success(_)),
                content: match completion {
                    ToolCompletion::Success(output) => output.content().to_owned(),
                    ToolCompletion::Failure(failure) => failure.message().to_owned(),
                    ToolCompletion::Unavailable => format!("tool '{name}' is unavailable"),
                },
            },
            supplement: match completion {
                ToolCompletion::Success(output) => {
                    Message::tool_image_supplement(name, id, output.images().to_vec())
                }
                ToolCompletion::Failure(_) | ToolCompletion::Unavailable => None,
            },
        }
    }

    /// A commit writes every result before its supplements, in the same order.
    /// Other detached calls may finish later. Adjacency-only providers normalize
    /// those late results at conversion; they do not require a run-global barrier.
    pub(super) fn commit_all(outputs: impl IntoIterator<Item = Self>, history: &mut Vec<Message>) {
        let mut supplements = Vec::new();
        for output in outputs {
            history.push(Message::ToolResult(output.result));
            supplements.extend(output.supplement);
        }
        history.extend(supplements);
    }

    /// Terminal settlement keeps text pairing but drops undelivered images.
    pub(super) fn into_result(self) -> ToolResult {
        self.result
    }
}

impl From<ToolResult> for CompletedToolOutput {
    fn from(result: ToolResult) -> Self {
        Self {
            result,
            supplement: None,
        }
    }
}

/// Detached completions are owned here before cancellable event publication.
#[derive(Default)]
pub(super) struct PendingToolOutputs {
    finished: Vec<CompletedToolOutput>,
}

impl PendingToolOutputs {
    pub(super) fn park_completion(&mut self, name: &str, id: &str, completion: &ToolCompletion) {
        self.park_finished(CompletedToolOutput::new(name, id, completion));
    }

    pub(super) fn park_finished(&mut self, output: impl Into<CompletedToolOutput>) {
        self.finished.push(output.into());
    }

    pub(super) fn take_finished(&mut self) -> Vec<CompletedToolOutput> {
        std::mem::take(&mut self.finished)
    }

    pub(super) fn drain_finished(&mut self, history: &mut Vec<Message>) -> usize {
        let outputs = self.take_finished();
        let count = outputs.len();
        CompletedToolOutput::commit_all(outputs, history);
        count
    }

    pub(super) fn drain_interrupted(&mut self, history: &mut Vec<Message>) {
        history.extend(
            self.finished
                .drain(..)
                .map(|output| Message::ToolResult(output.into_result())),
        );
    }
}
