use std::num::NonZeroUsize;

use tokio::sync::mpsc;

use super::ToolMetadata;

/// Progress emitted during one tool invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolProgress {
    message: String,
    completed_units: Option<u64>,
    total_units: Option<u64>,
    metadata: ToolMetadata,
}

impl ToolProgress {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            completed_units: None,
            total_units: None,
            metadata: ToolMetadata::default(),
        }
    }

    pub fn units(mut self, completed: u64, total: u64) -> Self {
        self.completed_units = Some(completed);
        self.total_units = Some(total);
        self
    }

    pub fn metadata(mut self, metadata: ToolMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn text(&self) -> &str {
        &self.message
    }

    pub fn completed_units(&self) -> Option<u64> {
        self.completed_units
    }

    pub fn total_units(&self) -> Option<u64> {
        self.total_units
    }

    pub fn presentation(&self) -> &ToolMetadata {
        &self.metadata
    }
}

/// Sending side of a bounded tool-progress channel.
#[derive(Clone, Debug)]
pub struct ToolProgressSender {
    sender: mpsc::Sender<ToolProgress>,
}

impl ToolProgressSender {
    /// Sends progress with backpressure. Returns `false` if the host dropped it.
    pub async fn send(&self, progress: ToolProgress) -> bool {
        self.sender.send(progress).await.is_ok()
    }
}

/// Receiving side of a bounded tool-progress channel.
#[derive(Debug)]
pub struct ToolProgressReceiver {
    receiver: mpsc::Receiver<ToolProgress>,
}

impl ToolProgressReceiver {
    pub async fn recv(&mut self) -> Option<ToolProgress> {
        self.receiver.recv().await
    }

    pub(crate) fn try_recv(&mut self) -> Option<ToolProgress> {
        self.receiver.try_recv().ok()
    }

    pub(crate) fn poll_recv(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<ToolProgress>> {
        self.receiver.poll_recv(cx)
    }
}

pub fn tool_progress_channel(capacity: NonZeroUsize) -> (ToolProgressSender, ToolProgressReceiver) {
    let (sender, receiver) = mpsc::channel(capacity.get());
    (
        ToolProgressSender { sender },
        ToolProgressReceiver { receiver },
    )
}
