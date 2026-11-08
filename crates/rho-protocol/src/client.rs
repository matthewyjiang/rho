//! Frontend side of a connection: handshake, overlapping typed requests, and
//! the daemon's notifications.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
};

use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
};

use crate::{
    jsonrpc,
    methods::{
        AuthLoginFinishedNotification, AuthLoginFinishedParams, AuthLoginPromptNotification,
        AuthLoginPromptParams, Hello, Method, Notification, SessionDetachedNotification,
        SessionDetachedParams, SessionEventNotification, SessionEventParams, AUTH_LOGIN_FINISHED,
        AUTH_LOGIN_PROMPT, SESSION_DETACHED, SESSION_EVENT,
    },
    ClientInfo, ErrorCode, FrameError, HelloParams, HelloResponse, Message, MessageReader,
    MessageWriter, ParseError, Request, RequestId, Response, RpcError, VersionMismatch,
    PROTOCOL_VERSION,
};

/// A handshaken connection to the daemon.
///
/// Requests may overlap: [`Client::request`] takes `&self`, and each call
/// gets its own response, paired by id. Notifications arrive on the
/// [`Notifications`] that [`Client::handshake`] returns.
///
/// Dropping the client ends the connection: the daemon answers the requests
/// it has read, then closes, and [`Notifications::recv`] returns `None`.
pub struct Client {
    shared: Arc<Shared>,
    writes: mpsc::UnboundedSender<Write>,
    next_id: AtomicI64,
}

/// The daemon's notifications on one connection, in the order it sent them.
///
/// Unbounded, so the reader never waits on the caller: undrained
/// notifications cost this client's own memory. Drain them, or drop this to
/// discard them.
pub struct Notifications {
    receiver: mpsc::UnboundedReceiver<DaemonNotification>,
}

/// One notification from the daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DaemonNotification {
    SessionEvent(SessionEventParams),
    SessionDetached(SessionDetachedParams),
    LoginPrompt(AuthLoginPromptParams),
    LoginFinished(AuthLoginFinishedParams),
    /// A notification a newer daemon sends that this crate does not know.
    Unknown {
        method: String,
    },
}

/// Why a request or handshake failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Writing the request failed.
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("rho daemon closed the connection")]
    Closed,
    /// The daemon sent a line this client cannot read, so the client closed
    /// the connection.
    #[error("rho daemon sent an invalid message: {0}")]
    InvalidMessage(#[from] ParseError),
    /// The daemon and this client speak incompatible protocol majors. The
    /// message says which side to change.
    #[error("{0}")]
    VersionMismatch(VersionMismatch),
    /// The daemon answered with an error. A `null`-id error (the daemon
    /// could not read a line) fails every pending request this way.
    #[error("rho daemon rejected {method}: {error}")]
    Rpc {
        method: &'static str,
        error: RpcError,
    },
    #[error("could not encode {method} params: {source}")]
    EncodeParams {
        method: &'static str,
        source: serde_json::Error,
    },
    #[error("rho daemon sent a malformed {method} result: {source}")]
    DecodeResult {
        method: &'static str,
        source: serde_json::Error,
    },
}

impl ClientError {
    /// The protocol error code the daemon answered with, if it rejected the
    /// request with one this crate knows.
    pub fn error_code(&self) -> Option<ErrorCode> {
        match self {
            Self::Rpc { error, .. } => error.error_code(),
            _ => None,
        }
    }

    /// How the turn ended, when the daemon refused a prompt with
    /// [`ErrorCode::TurnEnded`] because its turn ended before it saved the
    /// prompt.
    pub fn turn_ended(&self) -> Option<crate::TurnOutcome> {
        match self {
            Self::Rpc { error, .. } if error.error_code() == Some(ErrorCode::TurnEnded) => {
                serde_json::from_value(error.data.clone()?).ok()
            }
            _ => None,
        }
    }
}

/// A message for the writer task, and where to report its write.
type Write = (Message, oneshot::Sender<Result<(), FrameError>>);

impl Client {
    /// Sends `rho/hello` and returns the connection once the daemon accepts
    /// this client's protocol, with its notifications.
    ///
    /// An incompatible daemon yields [`ClientError::VersionMismatch`], whether
    /// the daemon rejects the hello or reports an incompatible version.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime, because the connection's reader and writer
    /// run in spawned tasks.
    pub async fn handshake<S>(
        stream: S,
        client: ClientInfo,
    ) -> Result<(Self, Notifications, HelloResponse), ClientError>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::Open(HashMap::new())),
        });
        let (writes, write_queue) = mpsc::unbounded_channel();
        let (notify, receiver) = mpsc::unbounded_channel();
        tokio::spawn(read_messages(
            MessageReader::new(reader),
            Arc::clone(&shared),
            notify,
        ));
        tokio::spawn(write_messages(MessageWriter::new(writer), write_queue));
        let this = Self {
            shared,
            writes,
            next_id: AtomicI64::new(1),
        };
        let params = HelloParams {
            protocol: PROTOCOL_VERSION,
            client,
        };
        let hello = match this.request::<Hello>(&params).await {
            Ok(hello) => hello,
            Err(ClientError::Rpc { method, error })
                if error.error_code() == Some(ErrorCode::VersionMismatch) =>
            {
                let mismatch = error
                    .data
                    .clone()
                    .and_then(|data| serde_json::from_value::<VersionMismatch>(data).ok());
                return Err(match mismatch {
                    Some(mismatch) => ClientError::VersionMismatch(mismatch),
                    None => ClientError::Rpc { method, error },
                });
            }
            Err(error) => return Err(error),
        };
        if let Some(mismatch) = VersionMismatch::between(hello.protocol, PROTOCOL_VERSION) {
            return Err(ClientError::VersionMismatch(mismatch));
        }
        Ok((this, Notifications { receiver }, hello))
    }

    /// Sends one request and waits for its response. Requests may overlap;
    /// the daemon runs them concurrently, so send one that depends on
    /// another only after that one's response arrives.
    pub async fn request<M: Method>(&self, params: &M::Params) -> Result<M::Result, ClientError> {
        let method = M::NAME;
        let params = serde_json::to_value(params)
            .map_err(|source| ClientError::EncodeParams { method, source })?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, response) = oneshot::channel();
        self.shared.register(id, Waiter { method, reply })?;
        let request = Message::Request(Request {
            id: RequestId::Number(id),
            method: method.to_owned(),
            params: Some(params),
        });
        let (ack, written) = oneshot::channel();
        // The writer task finishes a write even if this call is dropped, so
        // a cancelled request never leaves half a line behind.
        let written = match self.writes.send((request, ack)) {
            Ok(()) => written.await.ok(),
            Err(_) => None,
        };
        match written {
            Some(Ok(())) => {}
            Some(Err(error)) => {
                self.shared.forget(id);
                return Err(error.into());
            }
            // The writer stopped after an earlier write broke the stream.
            None => {
                self.shared.forget(id);
                return Err(ClientError::Closed);
            }
        }
        let result = response.await.unwrap_or(Err(ClientError::Closed))?;
        serde_json::from_value(result)
            .map_err(|source| ClientError::DecodeResult { method, source })
    }
}

impl Notifications {
    /// The next notification, or `None` once the connection has closed and
    /// every notification it delivered was taken.
    pub async fn recv(&mut self) -> Option<DaemonNotification> {
        self.receiver.recv().await
    }
}

/// The requests awaiting responses, shared by callers and the reader task.
struct Shared {
    pending: Mutex<Pending>,
}

enum Pending {
    Open(HashMap<i64, Waiter>),
    /// The reader stopped. `None`: the daemon closed the connection. `Some`:
    /// it sent a line this client cannot read.
    Closed(Option<ParseError>),
}

/// One request awaiting its response.
struct Waiter {
    method: &'static str,
    reply: oneshot::Sender<Result<Value, ClientError>>,
}

impl Waiter {
    fn answer(self, outcome: Result<Value, RpcError>) {
        let method = self.method;
        // The caller may have stopped waiting; then nobody needs it.
        let _ = self
            .reply
            .send(outcome.map_err(|error| ClientError::Rpc { method, error }));
    }

    fn fail(self, reason: &Option<ParseError>) {
        let _ = self.reply.send(Err(closed_error(reason)));
    }
}

fn closed_error(reason: &Option<ParseError>) -> ClientError {
    match reason {
        None => ClientError::Closed,
        Some(error) => ClientError::InvalidMessage(error.clone()),
    }
}

impl Shared {
    fn register(&self, id: i64, waiter: Waiter) -> Result<(), ClientError> {
        match &mut *self.lock() {
            Pending::Open(waiters) => {
                waiters.insert(id, waiter);
                Ok(())
            }
            Pending::Closed(reason) => Err(closed_error(reason)),
        }
    }

    fn forget(&self, id: i64) {
        if let Pending::Open(waiters) = &mut *self.lock() {
            waiters.remove(&id);
        }
    }

    /// Answers request `id`; a response to no pending request is dropped.
    fn resolve(&self, id: i64, outcome: Result<Value, RpcError>) {
        let waiter = match &mut *self.lock() {
            Pending::Open(waiters) => waiters.remove(&id),
            Pending::Closed(_) => None,
        };
        if let Some(waiter) = waiter {
            waiter.answer(outcome);
        }
    }

    /// Fails every pending request with `error`: the daemon could not read
    /// a line, and cannot say whose.
    fn fail_all(&self, error: &RpcError) {
        let waiters = match &mut *self.lock() {
            Pending::Open(waiters) => std::mem::take(waiters),
            Pending::Closed(_) => HashMap::new(),
        };
        for waiter in waiters.into_values() {
            waiter.answer(Err(error.clone()));
        }
    }

    /// Fails every pending request, and every later one, with `reason`.
    fn close(&self, reason: Option<ParseError>) {
        let previous = std::mem::replace(&mut *self.lock(), Pending::Closed(reason.clone()));
        if let Pending::Open(waiters) = previous {
            for waiter in waiters.into_values() {
                waiter.fail(&reason);
            }
        }
    }

    /// Nothing panics while holding the lock, so a poisoned one is still
    /// consistent.
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Routes responses to their requests and notifications to `notify` until
/// the daemon closes the connection or sends a line this client cannot
/// read, then fails every pending request.
async fn read_messages<R: AsyncRead + Unpin>(
    mut reader: MessageReader<R>,
    shared: Arc<Shared>,
    notify: mpsc::UnboundedSender<DaemonNotification>,
) {
    let reason = loop {
        let line = match reader.read_line().await {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => break None,
        };
        match Message::parse(&line) {
            Ok(Message::Response(Response {
                id: Some(RequestId::Number(id)),
                outcome,
            })) => shared.resolve(id, outcome),
            Ok(Message::Response(Response {
                id: None,
                outcome: Err(error),
            })) => shared.fail_all(&error),
            // The daemon sends no requests, and this client numbers its own,
            // so neither a request nor any other response is for it.
            Ok(Message::Response(_) | Message::Request(_)) => {}
            Ok(Message::Notification(notification)) => match decode(notification) {
                // The caller may have dropped its notifications.
                Ok(notification) => {
                    let _ = notify.send(notification);
                }
                Err(error) => break Some(error),
            },
            Err(error) => break Some(error),
        }
    };
    shared.close(reason);
}

/// Decodes a notification this crate knows; a known one with params that
/// do not match is a daemon bug, which closes the connection.
fn decode(notification: jsonrpc::Notification) -> Result<DaemonNotification, ParseError> {
    let jsonrpc::Notification { method, params } = notification;
    if method == SESSION_EVENT {
        decode_params::<SessionEventNotification>(params).map(DaemonNotification::SessionEvent)
    } else if method == SESSION_DETACHED {
        decode_params::<SessionDetachedNotification>(params)
            .map(DaemonNotification::SessionDetached)
    } else if method == AUTH_LOGIN_PROMPT {
        decode_params::<AuthLoginPromptNotification>(params).map(DaemonNotification::LoginPrompt)
    } else if method == AUTH_LOGIN_FINISHED {
        decode_params::<AuthLoginFinishedNotification>(params)
            .map(DaemonNotification::LoginFinished)
    } else {
        Ok(DaemonNotification::Unknown { method })
    }
}

fn decode_params<N: Notification>(params: Option<Value>) -> Result<N::Params, ParseError> {
    serde_json::from_value(params.unwrap_or(Value::Null)).map_err(|_| ParseError::Invalid {
        id: None,
        reason: "notification params do not match the method",
    })
}

/// Writes requests in order, reporting each write, until the client is gone
/// or a write breaks the stream. Then it ends the stream, so the daemon
/// answers what it has read and closes the connection.
async fn write_messages<W: AsyncWrite + Unpin>(
    mut writer: MessageWriter<W>,
    mut writes: mpsc::UnboundedReceiver<Write>,
) {
    while let Some((message, ack)) = writes.recv().await {
        let written = writer.write(&message).await;
        // An I/O error may leave half a line behind; an oversized message
        // was refused before any of it was written.
        let broken = matches!(written, Err(FrameError::Io(_)));
        // The caller may have stopped waiting; the write happened anyway.
        let _ = ack.send(written);
        if broken {
            return;
        }
    }
    let _ = writer.shutdown().await;
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
