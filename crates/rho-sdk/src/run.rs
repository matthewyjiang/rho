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
        response: crate::HostInputResponse,
        accepted: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
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
    pub async fn steer(&self, input: crate::UserInput) -> Result<(), Error> {
        self.steer_retractable(input).await.map(drop)
    }

    /// Stages steering input and returns an ID that can retract it before application.
    pub async fn steer_retractable(&self, input: crate::UserInput) -> Result<SteeringId, Error> {
        let (accepted, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .send(RunCommand::Steer { input, accepted })
            .await
            .map_err(|_| Error::InvalidHostResponse {
                message: "run no longer accepts steering input".into(),
            })?;
        receipt.await.map_err(|_| Error::InvalidHostResponse {
            message: "run completed before accepting steering input".into(),
        })
    }

    /// Starts staging steering without waiting for the runtime acknowledgement.
    pub fn request_steer_retractable(
        &self,
        input: crate::UserInput,
    ) -> Result<impl Future<Output = Result<SteeringId, Error>> + Send + 'static, Error> {
        let (accepted, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .try_send(RunCommand::Steer { input, accepted })
            .map_err(|error| Error::InvalidHostResponse {
                message: format!("run cannot queue steering input: {error}"),
            })?;
        Ok(async move {
            receipt.await.map_err(|_| Error::InvalidHostResponse {
                message: "run completed before accepting steering input".into(),
            })
        })
    }

    /// Atomically retracts previously accepted steering if it is still staged.
    pub async fn retract_steering(&self, id: SteeringId) -> Result<SteeringRetraction, Error> {
        let (completed, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .send(RunCommand::RetractSteering { id, completed })
            .await
            .map_err(|_| Error::InvalidHostResponse {
                message: "run no longer accepts steering retractions".into(),
            })?;
        receipt.await.map_err(|_| Error::InvalidHostResponse {
            message: "run completed before processing steering retraction".into(),
        })
    }

    /// Starts a retraction request without waiting for its runtime-decided outcome.
    pub fn request_steering_retraction(
        &self,
        id: SteeringId,
    ) -> Result<impl Future<Output = Result<SteeringRetraction, Error>> + Send + 'static, Error>
    {
        let (completed, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .try_send(RunCommand::RetractSteering { id, completed })
            .map_err(|error| Error::InvalidHostResponse {
                message: format!("run cannot queue steering retraction: {error}"),
            })?;
        Ok(async move {
            receipt.await.map_err(|_| Error::InvalidHostResponse {
                message: "run completed before processing steering retraction".into(),
            })
        })
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

    pub fn id(&self) -> &RunId {
        &self.id
    }

    pub fn cancellation_handle(&self) -> CancellationToken {
        self.cancellation.clone()
    }

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
    pub async fn steer(&self, input: crate::UserInput) -> Result<(), Error> {
        self.steering_handle().steer(input).await
    }

    /// Stages steering input and returns an ID that can retract it before application.
    ///
    /// The returned opaque identifier remains stable and can be passed to
    /// [`Run::retract_steering`] while the input has not reached history.
    pub async fn steer_retractable(&self, input: crate::UserInput) -> Result<SteeringId, Error> {
        self.steering_handle().steer_retractable(input).await
    }

    /// Starts staging steering without waiting for the runtime acknowledgement.
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
    /// The runtime decides the race between applying and retracting the input
    /// when it processes this command. See [`SteeringRetraction`] for all
    /// possible outcomes.
    pub async fn retract_steering(&self, id: SteeringId) -> Result<SteeringRetraction, Error> {
        self.steering_handle().retract_steering(id).await
    }

    /// Starts a retraction request without waiting for its runtime-decided outcome.
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

    pub async fn respond(
        &self,
        request_id: crate::HostInputId,
        response: crate::HostInputResponse,
    ) -> Result<(), Error> {
        let (accepted, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .send(RunCommand::Respond {
                request_id,
                response,
                accepted,
            })
            .await
            .map_err(|_| Error::InvalidHostResponse {
                message: "run no longer accepts host input".into(),
            })?;
        receipt
            .await
            .map_err(|_| Error::InvalidHostResponse {
                message: "run completed before accepting host input".into(),
            })?
            .map_err(|message| Error::InvalidHostResponse { message })
    }

    /// Starts delivering a host response without waiting for the runtime acknowledgement.
    ///
    /// Event loops can continue consuming [`RunEvent`] values before awaiting the returned
    /// future, avoiding command/event backpressure cycles.
    pub fn request_respond(
        &self,
        request_id: crate::HostInputId,
        response: crate::HostInputResponse,
    ) -> Result<impl Future<Output = Result<(), Error>> + Send + 'static, Error> {
        let (accepted, receipt) = tokio::sync::oneshot::channel();
        self.commands
            .try_send(RunCommand::Respond {
                request_id,
                response,
                accepted,
            })
            .map_err(|error| Error::InvalidHostResponse {
                message: format!("run cannot queue host input: {error}"),
            })?;
        Ok(async move {
            receipt
                .await
                .map_err(|_| Error::InvalidHostResponse {
                    message: "run completed before accepting host input".into(),
                })?
                .map_err(|message| Error::InvalidHostResponse { message })
        })
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
