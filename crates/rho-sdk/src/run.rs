use std::future::Future;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    CancellationToken, Error, RunEvent, RunId, RunOutcome, SteeringId, SteeringRetraction,
};

pub(crate) enum RunCommand {
    Steer {
        input: crate::UserInput,
        accepted: tokio::sync::oneshot::Sender<SteeringId>,
    },
    RetractSteering {
        id: SteeringId,
        completed: tokio::sync::oneshot::Sender<SteeringRetraction>,
    },
    Respond {
        request_id: crate::HostInputId,
        /// The answers, or why the host gave none.
        response: Result<crate::HostInputResponse, crate::HostInputRefusal>,
        accepted: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

/// Maps a failed non-blocking command send: a closed channel means the run
/// ended, while a full one is backpressure the host may retry after draining
/// events.
fn queue_failure(error: mpsc::error::TrySendError<RunCommand>, command: &str) -> Error {
    match error {
        mpsc::error::TrySendError::Closed(_) => Error::RunFinished,
        mpsc::error::TrySendError::Full(_) => Error::InvalidHostResponse {
            message: format!("run cannot queue {command}: command queue is full"),
        },
    }
}

/// Cloneable port for staging steering into an active run.
///
/// Obtain one from [`Run::steering_handle`] before moving the [`Run`] into an
/// event pump. Hosts that only need to redirect work can keep this handle
/// without sharing the run or its event stream.
///
/// Workspace crates that name this type need a published `rho-sdk` that exports
/// it. Bump `rho-sdk` together with those consumers when this surface changes.
#[derive(Clone, Debug)]
pub struct SteeringHandle {
    commands: mpsc::Sender<RunCommand>,
}

impl SteeringHandle {
    /// Stages steering input for the next provider turn.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run ended before accepting it.
    pub async fn steer(&self, input: crate::UserInput) -> Result<(), Error> {
        self.steer_retractable(input).await.map(drop)
    }

    /// Stages steering input and returns an ID that can retract it before application.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run ended before accepting it.
    pub async fn steer_retractable(&self, input: crate::UserInput) -> Result<SteeringId, Error> {
        let (accepted, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .send(RunCommand::Steer { input, accepted })
            .await
            .map_err(|_| Error::RunFinished)?;
        receipt.await.map_err(|_| Error::RunFinished)
    }

    /// Starts staging steering without waiting for the runtime acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run has ended, or
    /// [`Error::InvalidHostResponse`] if its command queue is full. The returned
    /// future fails with [`Error::RunFinished`] if the run ends first.
    pub fn request_steer_retractable(
        &self,
        input: crate::UserInput,
    ) -> Result<impl Future<Output = Result<SteeringId, Error>> + Send + 'static, Error> {
        let (accepted, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .try_send(RunCommand::Steer { input, accepted })
            .map_err(|error| queue_failure(error, "steering input"))?;
        Ok(async move { receipt.await.map_err(|_| Error::RunFinished) })
    }

    /// Atomically retracts previously accepted steering if it is still staged.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run ended before processing it.
    pub async fn retract_steering(&self, id: SteeringId) -> Result<SteeringRetraction, Error> {
        let (completed, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .send(RunCommand::RetractSteering { id, completed })
            .await
            .map_err(|_| Error::RunFinished)?;
        receipt.await.map_err(|_| Error::RunFinished)
    }

    /// Starts a retraction request without waiting for its runtime-decided outcome.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run has ended, or
    /// [`Error::InvalidHostResponse`] if its command queue is full. The returned
    /// future fails with [`Error::RunFinished`] if the run ends first.
    pub fn request_steering_retraction(
        &self,
        id: SteeringId,
    ) -> Result<impl Future<Output = Result<SteeringRetraction, Error>> + Send + 'static, Error>
    {
        let (completed, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .try_send(RunCommand::RetractSteering { id, completed })
            .map_err(|error| queue_failure(error, "steering retraction"))?;
        Ok(async move { receipt.await.map_err(|_| Error::RunFinished) })
    }
}

/// Handle for one active SDK run and its ordered event stream.
pub struct Run {
    id: RunId,
    cancellation: CancellationToken,
    events: mpsc::Receiver<RunEvent>,
    commands: mpsc::Sender<RunCommand>,
    worker: Option<JoinHandle<Result<RunOutcome, Error>>>,
    finished: bool,
}

impl Run {
    pub(crate) fn new(
        id: RunId,
        cancellation: CancellationToken,
        events: mpsc::Receiver<RunEvent>,
        commands: mpsc::Sender<RunCommand>,
        worker: JoinHandle<Result<RunOutcome, Error>>,
    ) -> Self {
        Self {
            id,
            cancellation,
            events,
            commands,
            worker: Some(worker),
            finished: false,
        }
    }

    /// Identifier of this run.
    pub fn id(&self) -> &RunId {
        &self.id
    }

    /// Token that cancels this run cooperatively, shareable across tasks.
    pub fn cancellation_handle(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    /// Requests cooperative cancellation; the run ends with a cancelled outcome.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Cloneable port for steering this run after the [`Run`] moves into a pump.
    pub fn steering_handle(&self) -> SteeringHandle {
        SteeringHandle {
            commands: self.commands.clone(),
        }
    }

    /// Stages steering input for the next provider turn.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run ended before accepting it.
    pub async fn steer(&self, input: crate::UserInput) -> Result<(), Error> {
        self.steering_handle().steer(input).await
    }

    /// Stages steering input and returns an ID that can retract it before application.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run ended before accepting it.
    ///
    /// The returned opaque identifier remains stable and can be passed to
    /// [`Run::retract_steering`] while the input has not reached history.
    pub async fn steer_retractable(&self, input: crate::UserInput) -> Result<SteeringId, Error> {
        self.steering_handle().steer_retractable(input).await
    }

    /// Starts staging steering without waiting for the runtime acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run has ended, or
    /// [`Error::InvalidHostResponse`] if its command queue is full. The returned
    /// future fails with [`Error::RunFinished`] if the run ends first.
    ///
    /// This is useful for event loops that must continue consuming [`RunEvent`] values while
    /// the runtime processes the command. Await the returned future for the accepted ID.
    pub fn request_steer_retractable(
        &self,
        input: crate::UserInput,
    ) -> Result<impl Future<Output = Result<SteeringId, Error>> + Send + 'static, Error> {
        self.steering_handle().request_steer_retractable(input)
    }

    /// Atomically retracts previously accepted steering if it is still staged.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run ended before processing it.
    ///
    /// The runtime decides the race between applying and retracting the input
    /// when it processes this command. See [`SteeringRetraction`] for all
    /// possible outcomes.
    pub async fn retract_steering(&self, id: SteeringId) -> Result<SteeringRetraction, Error> {
        self.steering_handle().retract_steering(id).await
    }

    /// Starts a retraction request without waiting for its runtime-decided outcome.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run has ended, or
    /// [`Error::InvalidHostResponse`] if its command queue is full. The returned
    /// future fails with [`Error::RunFinished`] if the run ends first.
    ///
    /// Event loops can continue consuming [`RunEvent`] values before awaiting the returned
    /// future, avoiding command/event backpressure cycles.
    pub fn request_steering_retraction(
        &self,
        id: SteeringId,
    ) -> Result<impl Future<Output = Result<SteeringRetraction, Error>> + Send + 'static, Error>
    {
        self.steering_handle().request_steering_retraction(id)
    }

    /// Answers a [`RunEvent::ToolHostInputRequested`] request and waits until
    /// the runtime accepts it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidHostResponse`] if the runtime rejects the
    /// response, or [`Error::RunFinished`] if the run ended first.
    pub async fn respond(
        &self,
        request_id: crate::HostInputId,
        response: crate::HostInputResponse,
    ) -> Result<(), Error> {
        let (accepted, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .send(RunCommand::Respond {
                request_id,
                response: Ok(response),
                accepted,
            })
            .await
            .map_err(|_| Error::RunFinished)?;
        receipt
            .await
            .map_err(|_| Error::RunFinished)?
            .map_err(|message| Error::InvalidHostResponse { message })
    }

    /// Starts delivering a host response without waiting for the runtime acknowledgement.
    ///
    /// Event loops can continue consuming [`RunEvent`] values before awaiting the returned
    /// future, avoiding command/event backpressure cycles.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run has ended, or
    /// [`Error::InvalidHostResponse`] if its command queue is full. The returned
    /// future fails with [`Error::InvalidHostResponse`] if the runtime rejects
    /// the response, or [`Error::RunFinished`] if the run ends first.
    pub fn request_respond(
        &self,
        request_id: crate::HostInputId,
        response: crate::HostInputResponse,
    ) -> Result<impl Future<Output = Result<(), Error>> + Send + 'static, Error> {
        self.queue_response(request_id, Ok(response))
    }

    /// Starts answering a host input request with no answers, like
    /// [`Run::request_respond`]. The asking tool receives
    /// [`Error::HostInputRefused`] with `refusal` and the run continues.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RunFinished`] if the run has ended, or
    /// [`Error::InvalidHostResponse`] if its command queue is full. The returned
    /// future fails with [`Error::InvalidHostResponse`] if the runtime rejects
    /// the response, or [`Error::RunFinished`] if the run ends first.
    pub fn request_refuse(
        &self,
        request_id: crate::HostInputId,
        refusal: crate::HostInputRefusal,
    ) -> Result<impl Future<Output = Result<(), Error>> + Send + 'static, Error> {
        self.queue_response(request_id, Err(refusal))
    }

    fn queue_response(
        &self,
        request_id: crate::HostInputId,
        response: Result<crate::HostInputResponse, crate::HostInputRefusal>,
    ) -> Result<impl Future<Output = Result<(), Error>> + Send + 'static, Error> {
        let (accepted, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .try_send(RunCommand::Respond {
                request_id,
                response,
                accepted,
            })
            .map_err(|error| queue_failure(error, "host input"))?;
        Ok(async move {
            receipt
                .await
                .map_err(|_| Error::RunFinished)?
                .map_err(|message| Error::InvalidHostResponse { message })
        })
    }

    /// Receives the next ordered event, or `None` once the run has finished.
    pub async fn next_event(&mut self) -> Option<RunEvent> {
        self.events.recv().await
    }

    /// Waits for the run's final outcome, draining any unconsumed events.
    ///
    /// Cancel-safe: dropping this future before it resolves leaves the run
    /// intact, so a later call (or dropping the [`Run`]) still owns the worker.
    ///
    /// # Errors
    ///
    /// Returns the run's own failure, [`Error::Interrupted`] if the run task
    /// panicked or was aborted, or [`Error::RunFinished`] if the outcome was
    /// already consumed.
    pub async fn outcome(&mut self) -> Result<RunOutcome, Error> {
        let worker = self.worker.as_mut().ok_or(Error::RunFinished)?;
        let joined = loop {
            tokio::select! {
                joined = &mut *worker => break joined,
                event = self.events.recv() => {
                    if event.is_none() {
                        break (&mut *worker).await;
                    }
                }
            }
        };
        self.worker = None;
        self.finished = true;
        joined.map_err(|error| Error::Interrupted {
            message: format!("run task failed: {error}"),
        })?
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
