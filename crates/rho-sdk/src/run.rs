use tokio::{sync::mpsc, task::JoinHandle};

use crate::{CancellationToken, Error, RunEvent, RunId, RunOutcome};

/// Handle for one active SDK run and its ordered event stream.
pub struct Run {
    id: RunId,
    cancellation: CancellationToken,
    events: mpsc::Receiver<RunEvent>,
    worker: Option<JoinHandle<Result<RunOutcome, Error>>>,
    finished: bool,
}

impl Run {
    pub(crate) fn new(
        id: RunId,
        cancellation: CancellationToken,
        events: mpsc::Receiver<RunEvent>,
        worker: JoinHandle<Result<RunOutcome, Error>>,
    ) -> Self {
        Self {
            id,
            cancellation,
            events,
            worker: Some(worker),
            finished: false,
        }
    }

    pub fn id(&self) -> &RunId {
        &self.id
    }

    pub fn cancellation_handle(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub async fn next_event(&mut self) -> Option<RunEvent> {
        self.events.recv().await
    }

    pub async fn outcome(&mut self) -> Result<RunOutcome, Error> {
        let mut worker = self
            .worker
            .take()
            .ok_or_else(|| Error::InvalidHostResponse {
                message: "run outcome was already consumed".into(),
            })?;
        let result = loop {
            tokio::select! {
                result = &mut worker => {
                    break result.map_err(|error| Error::Interrupted {
                        message: format!("run task failed: {error}"),
                    })?;
                }
                event = self.events.recv() => {
                    if event.is_none() {
                        break worker.await.map_err(|error| Error::Interrupted {
                            message: format!("run task failed: {error}"),
                        })?;
                    }
                }
            }
        };
        self.finished = true;
        result
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.cancellation.cancel();
        if let Some(worker) = &self.worker {
            worker.abort();
        }
    }
}

impl std::fmt::Debug for Run {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Run")
            .field("id", &self.id)
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}
