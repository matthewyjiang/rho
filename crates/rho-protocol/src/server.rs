//! Daemon side of a connection: the handshake gate, concurrent dispatch,
//! session forwarding, and the single writer every outgoing message goes
//! through.

use std::{future::Future, sync::Arc};

use serde::Serialize;
use serde_json::{Map, Value};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, OwnedSemaphorePermit},
    task::JoinSet,
};

use crate::{
    forward::Forwarders,
    jsonrpc,
    methods::{
        Hello, JobList, JobListParams, JobListResponse, Method, SessionAttach, SessionAttachParams,
        SessionAttachResponse, SessionCancel, SessionCancelParams, SessionCancelResponse,
        SessionDetach, SessionDetachParams, SessionDetachResponse, SessionNew, SessionNewParams,
        SessionNewResponse, SessionPrompt, SessionPromptParams, SessionPromptResponse,
        SessionSteer, SessionSteerParams, SessionSteerResponse, Status, Stop, DAEMON_STATUS,
        DAEMON_STOP, HELLO, JOB_LIST, SESSION_ATTACH, SESSION_CANCEL, SESSION_DETACH, SESSION_NEW,
        SESSION_PROMPT, SESSION_STEER,
    },
    subscription::Subscription,
    DaemonInfo, DaemonStatus, ErrorCode, FrameError, HelloResponse, Message, MessageReader,
    MessageWriter, Request, RequestId, Response, RpcError, StatusParams, StopParams, StopResponse,
    VersionMismatch, PROTOCOL_VERSION,
};

/// The daemon's answers to post-handshake requests, one method per protocol
/// method.
///
/// [`serve_connection`] owns the protocol: framing, the hello gate, version
/// checks, params decoding, and concurrency. Implementors supply only daemon
/// behavior.
///
/// Each call runs in its own task, so calls overlap within a connection and
/// across connections; implementors synchronize their own state. Every call
/// the connection has read runs to completion, even after the peer leaves or
/// a write fails; only its answer may go unheard. A call should resolve once
/// its answer is known. Work that outlives a request belongs in a task the
/// implementor owns: a connection waits for its calls before it closes, even
/// at shutdown, so a call that never resolves keeps its connection, and a
/// stopping daemon, open.
///
/// A fallible method's error is sent to the client as is; use the
/// [`ErrorCode`] that names the failure.
///
/// `rho/session/detach` has no method: the connection answers it by
/// dropping that session's [`Subscription`], which the publisher sees as
/// [`crate::Published::Gone`].
pub trait Service: Send + Sync {
    /// Answers `rho/daemon/status`.
    fn status(&self) -> impl Future<Output = DaemonStatus> + Send;

    /// Handles `rho/daemon/stop`: begin a graceful shutdown, typically by
    /// resolving the `shutdown` future passed to [`serve_connection`]. The
    /// acknowledgement is written once this resolves, and that shutdown does
    /// not cancel it.
    fn stop(&self) -> impl Future<Output = ()> + Send;

    /// Handles `rho/session/new`: open a session and register its job.
    fn new_session(
        &self,
        params: SessionNewParams,
    ) -> impl Future<Output = Result<SessionNewResponse, RpcError>> + Send;

    /// Handles `rho/session/prompt`: start a turn the service runs in a task
    /// of its own, resolving once the turn has saved its prompt (see
    /// [`SessionPromptResponse`]). The turn outlives the request and the
    /// connection.
    fn prompt_session(
        &self,
        params: SessionPromptParams,
    ) -> impl Future<Output = Result<SessionPromptResponse, RpcError>> + Send;

    /// Handles `rho/session/cancel`: ask the session's turn in progress to
    /// stop, without waiting for it to end.
    fn cancel_session(
        &self,
        params: SessionCancelParams,
    ) -> impl Future<Output = Result<SessionCancelResponse, RpcError>> + Send;

    /// Handles `rho/session/steer`: queue input for the running turn,
    /// without waiting for the turn to apply it.
    fn steer_session(
        &self,
        params: SessionSteerParams,
    ) -> impl Future<Output = Result<SessionSteerResponse, RpcError>> + Send;

    /// Handles `rho/session/attach`: subscribe this connection to the
    /// session's events (see [`crate::subscription()`]). The connection sends
    /// the response, then forwards the subscription's events until the
    /// publisher ends it, the client detaches or attaches the session again,
    /// or the connection ends. Events published before this resolves follow
    /// the response.
    fn attach_session(
        &self,
        params: SessionAttachParams,
    ) -> impl Future<Output = Result<(SessionAttachResponse, Subscription), RpcError>> + Send;

    /// Answers `rho/job/list`.
    fn list_jobs(&self) -> impl Future<Output = JobListResponse> + Send;
}

/// What the daemon reports in its `rho/hello` response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    pub daemon: DaemonInfo,
    pub capabilities: Vec<String>,
}

/// Messages queued for the connection's writer, written in queue order.
///
/// Responses are unbounded: each answers a request the peer sent, so they
/// grow only while the peer sends requests faster than it reads the answers.
/// The socket is private to the daemon's user, so only a buggy frontend does
/// that, and it costs memory rather than correctness. A request's answer
/// never waits behind events.
///
/// Notifications are bounded: each holds one of the connection's
/// [`crate::forward::NOTIFICATION_QUEUE`] permits until written, so a client
/// that stops reading stalls its forwarders, and their publishers report
/// the lag, instead of growing this queue. One queue for both keeps write
/// order equal to queue order, which the forwarders' ordering rests on.
pub(crate) type Outbox = mpsc::UnboundedSender<Outgoing>;

/// One message for the connection's writer.
pub(crate) enum Outgoing {
    Response(Response),
    /// Holds its queue permit until written.
    Notification(jsonrpc::Notification, OwnedSemaphorePermit),
}

/// Serves one connection until the peer stops sending or `shutdown`
/// resolves.
///
/// Requests are read in order, and `rho/hello` is answered before the next
/// line is read, so nothing reaches `service` until the handshake succeeds.
/// After it, each service request runs in its own task and is answered when
/// it completes: a slow request does not hold up later ones, and responses
/// may arrive out of order, paired with their requests by id. Every outgoing
/// message goes through one writer, so frames never interleave.
///
/// Requests also *run* concurrently, so a client that needs one request to
/// take effect before another sends the second only after the first's
/// response arrives.
///
/// `rho/session/attach` answers, then forwards the session's events as
/// `rho/session/event` notifications, and `rho/session/detached` when the
/// publisher closes with a reason. Attaching a session again replaces its
/// forwarder; `rho/session/detach` stops it. Events a forwarder queued come
/// before the response that replaced or stopped it, and none come after.
///
/// When the peer stops sending, a write fails, or `shutdown` resolves, the
/// connection stops reading, lets every request it has already read run to
/// completion, writes the responses it still can, and closes. That is how
/// the `rho/daemon/stop` acknowledgement reaches the client although the
/// stop itself resolves `shutdown`. Its forwarders send what their
/// publishers have already published, and the reason of each publisher
/// already closed, then end; they never wait for more, so a service that
/// closes its publishers before `shutdown` resolves tells every attached
/// client why it was detached.
///
/// Recoverable protocol errors (bad JSON, invalid messages, unknown methods)
/// get an error response and the connection continues; framing errors end it
/// once the requests already read are answered.
///
/// # Panics
///
/// Outside a Tokio runtime, because each request runs in a spawned task.
pub async fn serve_connection<S, Svc>(
    stream: S,
    info: &ServerInfo,
    service: Arc<Svc>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), FrameError>
where
    S: AsyncRead + AsyncWrite,
    Svc: Service + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    let (outbox, inbox) = mpsc::unbounded_channel();
    let mut calls = JoinSet::new();
    let forwarders = Arc::new(Forwarders::new());
    // The writer ends once every sender is gone (the reader's when it stops
    // reading, each forwarder's when the reader stops them, and each call's
    // when that call has answered) or a write fails.
    let (read, write) = tokio::join!(
        async {
            let read = read_requests(
                MessageReader::new(reader),
                info,
                service,
                outbox,
                shutdown,
                &mut calls,
                &forwarders,
            )
            .await;
            // Unlike calls, forwarders end with the connection.
            forwarders.end_all().await;
            read
        },
        write_messages(MessageWriter::new(writer), inbox),
    );
    // After a failed write, calls may still be running; their answers go
    // nowhere, but they finish.
    while calls.join_next().await.is_some() {}
    read.and(write)
}

/// Reads requests and routes each one until the peer stops sending,
/// `shutdown` resolves, the writer is gone, or the framing breaks. Dropping
/// `outbox` on return lets the writer finish.
async fn read_requests<R, Svc>(
    mut reader: MessageReader<R>,
    info: &ServerInfo,
    service: Arc<Svc>,
    outbox: Outbox,
    shutdown: impl Future<Output = ()>,
    calls: &mut JoinSet<()>,
    forwarders: &Arc<Forwarders>,
) -> Result<(), FrameError>
where
    R: AsyncRead + Unpin,
    Svc: Service + 'static,
{
    let mut shutdown = std::pin::pin!(shutdown);
    let mut connection = Connection::AwaitingHello;
    loop {
        // `read_line` is not cancel-safe, so only branches that end the
        // connection may race it.
        let line = tokio::select! {
            biased;
            () = &mut shutdown => return Ok(()),
            // The writer stops early only when a write failed.
            () = outbox.closed() => return Ok(()),
            line = reader.read_line() => line,
        };
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(error @ FrameError::LineTooLong { .. }) => {
                // Best effort: tell the peer why the connection is closing.
                let _ = outbox.send(Outgoing::Response(Response {
                    id: None,
                    outcome: Err(RpcError::new(ErrorCode::InvalidRequest, error.to_string())),
                }));
                return Err(error);
            }
            Err(error @ FrameError::Io(_)) => return Err(error),
        };
        let response = match Message::parse(&line) {
            Ok(Message::Request(Request { id, method, params })) => {
                match connection.route(&method, params, info) {
                    Ok(Route::Answered(result)) => Response {
                        id: Some(id),
                        outcome: Ok(result),
                    },
                    Ok(Route::Call(call)) => {
                        let responder = Responder {
                            id: Some(id),
                            outbox: outbox.clone(),
                        };
                        let service = Arc::clone(&service);
                        let forwarders = Arc::clone(forwarders);
                        calls.spawn(
                            async move { call.run(&*service, responder, &forwarders).await },
                        );
                        // A finished call stays in the set until joined.
                        while calls.try_join_next().is_some() {}
                        continue;
                    }
                    Ok(Route::Detach(SessionDetachParams { session })) => {
                        forwarders.detach(&session);
                        Response {
                            id: Some(id),
                            outcome: encode_result::<SessionDetach>(&SessionDetachResponse {}),
                        }
                    }
                    Err(error) => Response {
                        id: Some(id),
                        outcome: Err(error),
                    },
                }
            }
            // Clients send no notifications and the daemon sends no
            // requests, so neither needs a reply.
            Ok(Message::Notification(_) | Message::Response(_)) => continue,
            Err(error) => error.to_response(),
        };
        // A send fails only once the writer is gone, which the next
        // iteration observes.
        let _ = outbox.send(Outgoing::Response(response));
    }
}

/// Writes queued messages in order until every sender is gone or a write
/// fails. Returning drops `inbox`, which tells the reader to stop.
async fn write_messages<W: AsyncWrite + Unpin>(
    mut writer: MessageWriter<W>,
    mut inbox: mpsc::UnboundedReceiver<Outgoing>,
) -> Result<(), FrameError> {
    while let Some(outgoing) = inbox.recv().await {
        match outgoing {
            Outgoing::Response(response) => writer.write(&Message::Response(response)).await?,
            // The permit returns to the queue once the line is written.
            Outgoing::Notification(notification, _permit) => {
                writer.write(&Message::Notification(notification)).await?;
            }
        }
    }
    Ok(())
}

/// Handshake state of one connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Connection {
    AwaitingHello,
    Ready,
}

/// Where one request's answer comes from.
enum Route {
    /// The protocol answered it in place, in read order.
    Answered(Value),
    /// The service answers it, in its own task.
    Call(ServiceCall),
    /// `rho/session/detach`: the connection stops that session's forwarder
    /// and answers in place.
    Detach(SessionDetachParams),
}

/// A post-handshake request whose params decoded, waiting for the service.
enum ServiceCall {
    Status,
    Stop,
    NewSession(SessionNewParams),
    PromptSession(SessionPromptParams),
    CancelSession(SessionCancelParams),
    SteerSession(SessionSteerParams),
    AttachSession(SessionAttachParams),
    ListJobs,
}

impl Connection {
    /// Answers the handshake, protocol errors, and detaches in place, and
    /// hands service methods to [`ServiceCall`].
    fn route(
        &mut self,
        method: &str,
        params: Option<Value>,
        info: &ServerInfo,
    ) -> Result<Route, RpcError> {
        match (*self, method) {
            (Self::AwaitingHello, HELLO) => {
                let hello = decode_params::<Hello>(params)?;
                if let Some(mismatch) = VersionMismatch::between(PROTOCOL_VERSION, hello.protocol) {
                    return Err(
                        RpcError::new(ErrorCode::VersionMismatch, mismatch.to_string())
                            .with_data(encode(&mismatch)?),
                    );
                }
                let response = encode_result::<Hello>(&HelloResponse {
                    protocol: PROTOCOL_VERSION,
                    daemon: info.daemon.clone(),
                    capabilities: info.capabilities.clone(),
                })?;
                *self = Self::Ready;
                Ok(Route::Answered(response))
            }
            (Self::AwaitingHello, method) => Err(RpcError::new(
                ErrorCode::HandshakeRequired,
                format!("send {HELLO} before {method}"),
            )),
            (Self::Ready, HELLO) => Err(RpcError::new(
                ErrorCode::InvalidRequest,
                format!("{HELLO} was already accepted on this connection"),
            )),
            (Self::Ready, SESSION_DETACH) => {
                decode_params::<SessionDetach>(params).map(Route::Detach)
            }
            (Self::Ready, method) => ServiceCall::decode(method, params).map(Route::Call),
        }
    }
}

impl ServiceCall {
    /// Decodes a post-handshake request for the service, or refuses a method
    /// the protocol does not define.
    fn decode(method: &str, params: Option<Value>) -> Result<Self, RpcError> {
        match method {
            DAEMON_STATUS => decode_params::<Status>(params).map(|StatusParams {}| Self::Status),
            DAEMON_STOP => decode_params::<Stop>(params).map(|StopParams {}| Self::Stop),
            SESSION_NEW => decode_params::<SessionNew>(params).map(Self::NewSession),
            SESSION_PROMPT => decode_params::<SessionPrompt>(params).map(Self::PromptSession),
            SESSION_CANCEL => decode_params::<SessionCancel>(params).map(Self::CancelSession),
            SESSION_STEER => decode_params::<SessionSteer>(params).map(Self::SteerSession),
            SESSION_ATTACH => decode_params::<SessionAttach>(params).map(Self::AttachSession),
            JOB_LIST => decode_params::<JobList>(params).map(|JobListParams {}| Self::ListJobs),
            method => Err(RpcError::new(
                ErrorCode::MethodNotFound,
                format!("unknown method {method}"),
            )),
        }
    }

    /// Runs the call and answers it through `responder`; an attach then
    /// starts forwarding through `forwarders`.
    async fn run<Svc: Service>(
        self,
        service: &Svc,
        responder: Responder,
        forwarders: &Arc<Forwarders>,
    ) {
        let outcome = match self {
            Self::AttachSession(params) => {
                let session = params.session.clone();
                let (response, subscription) = match service.attach_session(params).await {
                    Ok(attached) => attached,
                    Err(error) => return responder.send(Err(error)),
                };
                let result = match encode_result::<SessionAttach>(&response) {
                    Ok(result) => result,
                    Err(error) => return responder.send(Err(error)),
                };
                let outbox = responder.outbox.clone();
                forwarders.attach(session, subscription, outbox, move || {
                    responder.send(Ok(result));
                });
                return;
            }
            Self::Status => encode_result::<Status>(&service.status().await),
            Self::Stop => {
                service.stop().await;
                encode_result::<Stop>(&StopResponse {})
            }
            Self::NewSession(params) => {
                encode_outcome::<SessionNew>(service.new_session(params).await)
            }
            Self::PromptSession(params) => {
                encode_outcome::<SessionPrompt>(service.prompt_session(params).await)
            }
            Self::CancelSession(params) => {
                encode_outcome::<SessionCancel>(service.cancel_session(params).await)
            }
            Self::SteerSession(params) => {
                encode_outcome::<SessionSteer>(service.steer_session(params).await)
            }
            Self::ListJobs => encode_result::<JobList>(&service.list_jobs().await),
        };
        responder.send(outcome);
    }
}

/// The duty to answer one request handed to the service.
///
/// A call that ends without answering, because its handler panicked or the
/// connection cancelled it, answers with an internal error on drop, so the
/// peer never waits on a request that will not finish.
struct Responder {
    /// `None` once answered.
    id: Option<RequestId>,
    outbox: Outbox,
}

impl Responder {
    fn send(mut self, outcome: Result<Value, RpcError>) {
        self.answer(outcome);
    }

    fn answer(&mut self, outcome: Result<Value, RpcError>) {
        if let Some(id) = self.id.take() {
            // Fails only once the writer is gone; then nobody can hear it.
            let _ = self.outbox.send(Outgoing::Response(Response {
                id: Some(id),
                outcome,
            }));
        }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.answer(Err(RpcError::new(
            ErrorCode::InternalError,
            "the daemon failed while handling this request",
        )));
    }
}

/// Decodes `M`'s params. Absent params decode like `{}`, so params-less
/// methods accept both. Typing by [`Method`] keeps the server on the same
/// params type the client sends.
fn decode_params<M: Method>(params: Option<Value>) -> Result<M::Params, RpcError> {
    serde_json::from_value(params.unwrap_or_else(|| Value::Object(Map::new())))
        .map_err(|error| RpcError::new(ErrorCode::InvalidParams, error.to_string()))
}

/// Encodes `M`'s result, the type [`crate::Client::request`] decodes.
fn encode_result<M: Method>(result: &M::Result) -> Result<Value, RpcError> {
    encode(result)
}

/// Encodes a fallible call's outcome as `M`'s result.
fn encode_outcome<M: Method>(outcome: Result<M::Result, RpcError>) -> Result<Value, RpcError> {
    encode_result::<M>(&outcome?)
}

/// Fails only for values JSON cannot hold, such as a non-UTF-8 path.
fn encode<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value)
        .map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
