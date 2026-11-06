//! Newline-delimited JSON-RPC 2.0 framing.
//!
//! Each message is one compact JSON object followed by `\n`. Compact JSON never
//! contains a raw newline (strings escape it), so a line is always exactly one
//! message. [`MessageReader`] and [`MessageWriter`] work on any
//! `AsyncRead`/`AsyncWrite` half, so the same code runs over a Unix socket, a
//! pipe, or an in-memory duplex in tests.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::ErrorCode;

/// Longest line, in bytes without the trailing newline, that either side
/// reads or writes.
///
/// A tripwire, not a budget: protocol 1.0 messages are under 1 KiB. The limit
/// only stops a peer that never sends a newline from growing the read buffer
/// without bound. If a real message ever reaches it, raise the limit rather
/// than chunking the message.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

const JSONRPC_VERSION: &str = "2.0";

/// Identifier that pairs a response with its request.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// Integer identifier. Rho clients number requests from 1.
    Number(i64),
    /// String identifier, accepted for JSON-RPC compatibility.
    String(String),
}

impl RequestId {
    fn from_value(value: Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(Self::Number),
            Value::String(string) => Some(Self::String(string)),
            Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Number(number) => Value::from(*number),
            Self::String(string) => Value::String(string.clone()),
        }
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(formatter, "{number}"),
            Self::String(string) => write!(formatter, "{string:?}"),
        }
    }
}

/// A call that expects exactly one [`Response`] with the same id.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: RequestId,
    pub method: String,
    /// Absent params are sent as no `params` member, not `null`.
    pub params: Option<Value>,
}

/// A one-way message. The receiver never replies, even with an error.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub method: String,
    pub params: Option<Value>,
}

/// The reply to one [`Request`].
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// `None` is the JSON-RPC `null` id, used when the request that failed
    /// could not be parsed far enough to read its id.
    pub id: Option<RequestId>,
    pub outcome: Result<Value, RpcError>,
}

/// A JSON-RPC error object.
///
/// `code` stays a raw integer so a client can carry codes added by a newer
/// protocol minor; [`RpcError::error_code`] maps known ones to [`ErrorCode`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// An error with a known code and no data.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.code(),
            message: message.into(),
            data: None,
        }
    }

    /// Attaches structured data that clients can decode into a typed error.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// The known error code, or `None` for a code this crate does not define.
    pub fn error_code(&self) -> Option<ErrorCode> {
        ErrorCode::from_code(self.code)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// Any JSON-RPC 2.0 message.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

impl Message {
    /// Parses one line (without its newline) into a message.
    ///
    /// A [`ParseError`] is recoverable: the line framing is intact, so a
    /// server replies with [`ParseError::to_response`] and keeps reading.
    pub fn parse(line: &[u8]) -> Result<Self, ParseError> {
        let value = serde_json::from_slice::<Value>(line)
            .map_err(|error| ParseError::Json(error.to_string()))?;
        match value {
            Value::Object(object) => Self::from_object(object),
            _ => Err(ParseError::Invalid {
                id: None,
                reason: "a message must be a JSON object",
            }),
        }
    }

    fn from_object(mut object: Map<String, Value>) -> Result<Self, ParseError> {
        // Outer `None`: no `id` member. Inner `None`: `"id": null`.
        let id = match object.remove("id") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(raw) => match RequestId::from_value(raw) {
                Some(id) => Some(Some(id)),
                None => {
                    return Err(ParseError::Invalid {
                        id: None,
                        reason: "id must be a string or an integer",
                    })
                }
            },
        };
        let echo_id = id.clone().flatten();
        let invalid = |reason: &'static str| ParseError::Invalid {
            id: echo_id.clone(),
            reason,
        };
        if object.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC_VERSION) {
            return Err(invalid("jsonrpc must be \"2.0\""));
        }
        if let Some(method) = object.remove("method") {
            let Value::String(method) = method else {
                return Err(invalid("method must be a string"));
            };
            let params = object.remove("params");
            if params
                .as_ref()
                .is_some_and(|params| !params.is_object() && !params.is_array())
            {
                return Err(invalid("params must be an object or an array"));
            }
            return match id {
                None => Ok(Self::Notification(Notification { method, params })),
                Some(Some(id)) => Ok(Self::Request(Request { id, method, params })),
                Some(None) => Err(invalid("a request id must not be null")),
            };
        }
        let Some(id) = id else {
            return Err(invalid("a response must have an id"));
        };
        let outcome = match (object.remove("result"), object.remove("error")) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(serde_json::from_value::<RpcError>(error)
                .map_err(|_| invalid("error must have an integer code and a string message"))?),
            (Some(_), Some(_)) | (None, None) => {
                return Err(invalid(
                    "a response must have exactly one of result and error",
                ))
            }
        };
        Ok(Self::Response(Response { id, outcome }))
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("jsonrpc".into(), Value::from(JSONRPC_VERSION));
        match self {
            Self::Request(Request { id, method, params }) => {
                object.insert("id".into(), id.to_value());
                object.insert("method".into(), Value::from(method.as_str()));
                if let Some(params) = params {
                    object.insert("params".into(), params.clone());
                }
            }
            Self::Notification(Notification { method, params }) => {
                object.insert("method".into(), Value::from(method.as_str()));
                if let Some(params) = params {
                    object.insert("params".into(), params.clone());
                }
            }
            Self::Response(Response { id, outcome }) => {
                object.insert(
                    "id".into(),
                    id.as_ref().map_or(Value::Null, RequestId::to_value),
                );
                match outcome {
                    Ok(result) => object.insert("result".into(), result.clone()),
                    Err(error) => object.insert(
                        "error".into(),
                        serde_json::to_value(error).expect("an RpcError always serializes to JSON"),
                    ),
                };
            }
        }
        Value::Object(object)
    }
}

/// A line that is not a valid JSON-RPC 2.0 message.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The line is not JSON.
    #[error("invalid JSON: {0}")]
    Json(String),
    /// The line is JSON but not a JSON-RPC 2.0 message.
    #[error("invalid JSON-RPC message: {reason}")]
    Invalid {
        /// The message's id, when it was readable, so the error reply can
        /// still be paired with the request.
        id: Option<RequestId>,
        reason: &'static str,
    },
}

impl ParseError {
    /// The error reply JSON-RPC prescribes for this line.
    pub fn to_response(&self) -> Response {
        let (id, code) = match self {
            Self::Json(_) => (None, ErrorCode::ParseError),
            Self::Invalid { id, .. } => (id.clone(), ErrorCode::InvalidRequest),
        };
        Response {
            id,
            outcome: Err(RpcError::new(code, self.to_string())),
        }
    }
}

/// A failure that breaks the line framing; the connection cannot continue.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// A line reached [`MAX_LINE_BYTES`]. When reading, `observed` is how many
    /// bytes arrived without a newline before the reader gave up.
    #[error(
        "JSON-RPC line of at least {observed} bytes exceeds the {limit}-byte limit (MAX_LINE_BYTES)"
    )]
    LineTooLong { limit: usize, observed: usize },
    #[error("JSON-RPC stream I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// A closing connection's peer did not take a message within `grace`
    /// ([`crate::CLOSE_GRACE`]), so the rest went unsent.
    #[error("the peer stopped reading; a write did not finish within CLOSE_GRACE ({grace:?})")]
    Stalled { grace: std::time::Duration },
}

/// Reads newline-delimited messages from one stream half.
pub struct MessageReader<R> {
    inner: BufReader<R>,
}

impl<R: AsyncRead + Unpin> MessageReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            inner: BufReader::new(reader),
        }
    }

    /// Reads the next line without its newline (and without a trailing `\r`).
    ///
    /// Returns `Ok(None)` at end of stream. A final line without a newline is
    /// still returned. Lines over [`MAX_LINE_BYTES`] fail without being
    /// buffered in full.
    pub async fn read_line(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let mut line = Vec::new();
        loop {
            let (consumed, complete) = {
                let available = self.inner.fill_buf().await?;
                if available.is_empty() {
                    return Ok((!line.is_empty()).then_some(line));
                }
                let newline = available.iter().position(|byte| *byte == b'\n');
                let chunk = &available[..newline.unwrap_or(available.len())];
                let observed = line.len() + chunk.len();
                if observed > MAX_LINE_BYTES {
                    return Err(FrameError::LineTooLong {
                        limit: MAX_LINE_BYTES,
                        observed,
                    });
                }
                line.extend_from_slice(chunk);
                match newline {
                    Some(index) => (index + 1, true),
                    None => (available.len(), false),
                }
            };
            self.inner.consume(consumed);
            if complete {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
        }
    }
}

/// Writes newline-delimited messages to one stream half.
pub struct MessageWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin> MessageWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { inner: writer }
    }

    /// Writes and flushes one message. Refuses a message the peer's reader
    /// would reject, so the failure names this side instead of the peer.
    pub async fn write(&mut self, message: &Message) -> Result<(), FrameError> {
        let mut line =
            serde_json::to_vec(&message.to_value()).expect("a serde_json::Value always serializes");
        if line.len() > MAX_LINE_BYTES {
            return Err(FrameError::LineTooLong {
                limit: MAX_LINE_BYTES,
                observed: line.len(),
            });
        }
        line.push(b'\n');
        self.inner.write_all(&line).await?;
        self.inner.flush().await?;
        Ok(())
    }

    /// Ends the stream, so the peer reads end of stream once it has every
    /// message written before.
    pub(crate) async fn shutdown(&mut self) -> std::io::Result<()> {
        self.inner.shutdown().await
    }
}

#[cfg(test)]
#[path = "jsonrpc_tests.rs"]
mod tests;
