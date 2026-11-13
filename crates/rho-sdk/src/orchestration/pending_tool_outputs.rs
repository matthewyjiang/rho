use crate::{
    event::ToolCompletion,
    model::{ImageContent, Message, ToolResult},
};

/// One completion owns its textual result and its deliverable images together.
/// Schedulers may park it; only a normal commit delivers the images.
pub(super) struct CompletedToolOutput {
    result: ToolResult,
    /// Successful output images, attached to `result` when it commits.
    images: Vec<ImageContent>,
}

impl CompletedToolOutput {
    pub(super) fn new(name: &str, id: &str, completion: &ToolCompletion) -> Self {
        Self {
            result: completion.model_result(name, id),
            images: completion
                .output()
                .filter(|_| !completion.is_failure())
                .map(|output| output.images().to_vec())
                .unwrap_or_default(),
        }
    }

    /// Commits results with their images, in order. Other detached calls may
    /// finish later; adjacency-only providers normalize those late results at
    /// conversion, so a commit needs no run-global barrier.
    pub(super) fn commit_all(outputs: impl IntoIterator<Item = Self>, history: &mut Vec<Message>) {
        history.extend(
            outputs
                .into_iter()
                .map(|output| Message::ToolResult(output.result.with_images(output.images).into())),
        );
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
            images: Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "pending_tool_outputs_tests.rs"]
mod tests;

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
                .map(|output| Message::ToolResult(output.into_result().into())),
        );
    }
}
