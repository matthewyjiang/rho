//! Host-owned input collected at synchronized runtime boundaries.
use tokio::sync::{mpsc, oneshot};

use crate::{Error, RunId, SessionId, UserInput};

/// The runtime checkpoint requesting pending host input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputBoundary {
    /// Before a provider request, after the preceding tool batch has settled.
    BeforeProvider,
    /// Before committing an end-turn response. An empty reply closes this run's
    /// delivery window; later arrivals belong to the host's idle queue.
    BeforeCompletion,
}

/// Runtime half of an opt-in, host-serviced boundary input channel.
///
/// Hosts retain notification policy and queues. The runtime knows neither the
/// source nor the meaning of these inputs. Install a fresh channel on an idle
/// session for each run and service requests alongside its event stream.
#[derive(Clone)]
pub struct BoundaryInputSource {
    sender: mpsc::Sender<BoundaryInputRequest>,
}

/// One checkpoint, identified independently of human steering and host questions.
/// Dropping the request fails the run rather than silently allowing completion.
pub struct BoundaryInputRequest {
    session_id: SessionId,
    run_id: RunId,
    boundary: InputBoundary,
    response: oneshot::Sender<BoundaryReply>,
}

pub(crate) struct BoundaryReply {
    pub(crate) input: Option<UserInput>,
    pub(crate) accepted: oneshot::Sender<()>,
}

/// Creates a channel with room for the sole outstanding checkpoint of a run.
/// There is no notification capacity here: pending work stays in the host queues.
pub fn boundary_input_channel() -> (BoundaryInputSource, mpsc::Receiver<BoundaryInputRequest>) {
    let (sender, receiver) = mpsc::channel(1);
    (BoundaryInputSource { sender }, receiver)
}

impl BoundaryInputRequest {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }
    pub fn boundary(&self) -> InputBoundary {
        self.boundary
    }

    /// Hands the collected input to the runtime and waits for its acceptance.
    ///
    /// Sending is synchronous; the returned future waits for acknowledgement.
    /// This lets a host hold its publication gate through the reply send, then
    /// release that gate before awaiting acceptance.
    ///
    /// Returns false if the run disappeared or was cancelled before acceptance.
    /// Keep the drained batch reserved until this returns; restore it on false.
    /// Do not race this future against other host work after sending the reply.
    /// The runtime checkpoints nonempty input into `Session::history` before
    /// acknowledgement, without awaiting events or provider work. An accepted
    /// input remains recoverable through `Session::snapshot` if the run is then
    /// dropped or its event consumer disconnects. Disk persistence stays with
    /// the host, as with all SDK session commits.
    ///
    /// For `BeforeCompletion`, sending `None` is the finalization handoff. The
    /// host must leave arrivals after its collection snapshot for the next run.
    ///
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-sdk): represent internal boundary input with a typed history message instead of User blocks.
    /// The exhaustive public `Message` enum currently requires wire-compatible
    /// user-role blocks. Frame internal origin in the body and use
    /// `RunEvent::BoundaryInputApplied` rather than treating this as human input.
    pub fn respond(
        self,
        input: Option<UserInput>,
    ) -> impl std::future::Future<Output = bool> + Send {
        let (accepted, receiver) = oneshot::channel();
        let sent = self
            .response
            .send(BoundaryReply { input, accepted })
            .is_ok();
        async move { sent && receiver.await.is_ok() }
    }
}

impl BoundaryInputSource {
    pub(crate) async fn request(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        boundary: InputBoundary,
    ) -> Result<BoundaryReply, Error> {
        let (response, receiver) = oneshot::channel();
        self.sender
            .send(BoundaryInputRequest {
                session_id: session_id.clone(),
                run_id: run_id.clone(),
                boundary,
                response,
            })
            .await
            .map_err(|_| disconnected())?;
        receiver.await.map_err(|_| disconnected())
    }
}

fn disconnected() -> Error {
    Error::Interrupted {
        message: "boundary input host disconnected before acknowledging the checkpoint".into(),
    }
}

#[cfg(test)]
#[path = "boundary_input_tests.rs"]
mod tests;
