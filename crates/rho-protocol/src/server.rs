//! Daemon side of a connection: the handshake gate and method dispatch.

use std::future::Future;

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    methods::{DAEMON_STATUS, DAEMON_STOP, HELLO},
    DaemonInfo, DaemonStatus, ErrorCode, FrameError, HelloParams, HelloResponse, Message,
    MessageReader, MessageWriter, Request, Response, RpcError, StatusParams, StopParams,
    StopResponse, VersionMismatch, PROTOCOL_VERSION,
};

/// The daemon's answers to post-handshake requests.
///
/// [`serve_connection`] owns the protocol: framing, the hello gate, version
/// checks, and params decoding. Implementors supply only daemon behavior.
/// Methods run on the connection task and should return promptly; work that
/// outlives a request belongs in a task the implementor spawns.
pub trait Service: Send + Sync {
    /// Answers `rho/daemon/status`.
    fn status(&self) -> DaemonStatus;

    /// Handles `rho/daemon/stop`: begin a graceful shutdown. The connection
    /// writes the acknowledgement after this returns, so the shutdown must not
    /// cancel that write; it should resolve the `shutdown` future passed to
    /// [`serve_connection`], which is only polled between requests.
    fn stop(&self);
}

/// What the daemon reports in its `rho/hello` response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    pub daemon: DaemonInfo,
    pub capabilities: Vec<String>,
}

/// Serves one connection until the peer disconnects or `shutdown` resolves.
///
/// Requests are handled one at a time, and `shutdown` is only observed
/// between them, so a response that is being written always completes.
/// Recoverable protocol errors (bad JSON, invalid messages, unknown methods)
/// get an error response and the connection continues; framing errors end it.
pub async fn serve_connection<S, Svc>(
    stream: S,
    info: &ServerInfo,
    service: &Svc,
    shutdown: impl Future<Output = ()>,
) -> Result<(), FrameError>
where
    S: AsyncRead + AsyncWrite,
    Svc: Service + ?Sized,
{
    let (reader, writer) = tokio::io::split(stream);
    let mut reader = MessageReader::new(reader);
    let mut writer = MessageWriter::new(writer);
    let mut shutdown = std::pin::pin!(shutdown);
    let mut connection = Connection::AwaitingHello;
    loop {
        let line = tokio::select! {
            biased;
            () = &mut shutdown => return Ok(()),
            line = reader.read_line() => line,
        };
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(error @ FrameError::LineTooLong { .. }) => {
                // Best effort: tell the peer why the connection is closing.
                let response = Response {
                    id: None,
                    outcome: Err(RpcError::new(ErrorCode::InvalidRequest, error.to_string())),
                };
                let _ = writer.write(&Message::Response(response)).await;
                return Err(error);
            }
            Err(error @ FrameError::Io(_)) => return Err(error),
        };
        let response = match Message::parse(&line) {
            Ok(Message::Request(request)) => Response {
                id: Some(request.id.clone()),
                outcome: connection.dispatch(request, info, service),
            },
            // Protocol 1.0 defines no client notifications and the daemon
            // sends no requests, so neither needs a reply.
            Ok(Message::Notification(_) | Message::Response(_)) => continue,
            Err(error) => error.to_response(),
        };
        writer.write(&Message::Response(response)).await?;
    }
}

/// Handshake state of one connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Connection {
    AwaitingHello,
    Ready,
}

impl Connection {
    fn dispatch<Svc: Service + ?Sized>(
        &mut self,
        request: Request,
        info: &ServerInfo,
        service: &Svc,
    ) -> Result<Value, RpcError> {
        let Request { method, params, .. } = request;
        match (*self, method.as_str()) {
            (Self::AwaitingHello, HELLO) => {
                let hello: HelloParams = decode_params(params)?;
                if let Some(mismatch) = VersionMismatch::between(PROTOCOL_VERSION, hello.protocol) {
                    return Err(
                        RpcError::new(ErrorCode::VersionMismatch, mismatch.to_string())
                            .with_data(encode(&mismatch)?),
                    );
                }
                let response = encode(&HelloResponse {
                    protocol: PROTOCOL_VERSION,
                    daemon: info.daemon.clone(),
                    capabilities: info.capabilities.clone(),
                })?;
                *self = Self::Ready;
                Ok(response)
            }
            (Self::AwaitingHello, method) => Err(RpcError::new(
                ErrorCode::HandshakeRequired,
                format!("send {HELLO} before {method}"),
            )),
            (Self::Ready, HELLO) => Err(RpcError::new(
                ErrorCode::InvalidRequest,
                format!("{HELLO} was already accepted on this connection"),
            )),
            (Self::Ready, DAEMON_STATUS) => {
                let StatusParams {} = decode_params(params)?;
                encode(&service.status())
            }
            (Self::Ready, DAEMON_STOP) => {
                let StopParams {} = decode_params(params)?;
                service.stop();
                encode(&StopResponse {})
            }
            (Self::Ready, method) => Err(RpcError::new(
                ErrorCode::MethodNotFound,
                format!("unknown method {method}"),
            )),
        }
    }
}

/// Absent params decode like `{}`, so params-less methods accept both.
fn decode_params<P: DeserializeOwned>(params: Option<Value>) -> Result<P, RpcError> {
    serde_json::from_value(params.unwrap_or_else(|| Value::Object(Map::new())))
        .map_err(|error| RpcError::new(ErrorCode::InvalidParams, error.to_string()))
}

/// Fails only for values JSON cannot hold, such as a non-UTF-8 path.
fn encode<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value)
        .map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
