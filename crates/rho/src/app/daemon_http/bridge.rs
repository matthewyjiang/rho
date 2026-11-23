//! WebSocket ↔ local daemon NDJSON relay for `rho daemon serve --http`.
//!
//! Wire contract over WebSocket:
//! - Every WebSocket **text frame** carries one complete JSON-RPC 2.0 object
//!   (no trailing newline required; compact JSON emitted toward the client).
//! - Maximum message/frame size matches [`rho_protocol::MAX_LINE_BYTES`]
//!   (16 MiB).
//! - Binary frames are refused with Close code `Unsupported` (1003).
//! - A single incoming text frame is compacted to a single line before reaching
//!   the local daemon socket so pretty-printed JSON works cleanly and malformed
//!   input containing raw newlines can never smuggle a second JSON-RPC line.
//! - For the protocol methods that accept a `ClientEnvironment`, the bridge
//!   overlays the client's variables on top of the server host's environment
//!   (captured when the server starts), so remote clients on another OS can
//!   send `{}` or omit `environment` and still get the host's `PATH`/`HOME`.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use rho_protocol::{
    methods::{
        AUTH_CHOOSE_STORE, AUTH_KEY, AUTH_LIST, AUTH_LOGIN, AUTH_LOGOUT, SESSION_ATTACH,
        SESSION_NEW, SESSION_PROMPT,
    },
    ErrorCode, FrameError, MessageReader, Response, RpcError, MAX_LINE_BYTES,
};
use rho_sdk::CancellationToken;
use serde_json::{Map, Value};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};
use tokio_tungstenite::{
    tungstenite::{
        protocol::{
            frame::{coding::CloseCode, CloseFrame},
            WebSocketConfig,
        },
        Message as WsMessage,
    },
    WebSocketStream,
};

/// Bounded outgoing WebSocket queue so a stalled peer exerts backpressure on
/// the daemon socket rather than growing bridge memory without bound.
const OUTGOING_QUEUE: usize = 128;

/// Keepalive ping interval so reverse proxies and tunnels do not drop idle
/// attached connections.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Protocol methods whose `params` include `environment: ClientEnvironment`.
const ENVIRONMENT_METHODS: [&str; 8] = [
    SESSION_NEW,
    SESSION_PROMPT,
    SESSION_ATTACH,
    AUTH_LIST,
    AUTH_KEY,
    AUTH_LOGOUT,
    AUTH_LOGIN,
    AUTH_CHOOSE_STORE,
];

/// WebSocket configuration sized to match `rho-protocol`'s 16 MiB message cap.
pub(super) fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_LINE_BYTES))
        .max_frame_size(Some(MAX_LINE_BYTES))
}

/// Relays one authenticated WebSocket connection to `daemon` until either side
/// closes or `shutdown` is cancelled.
pub(super) async fn relay_websocket<S, D>(
    ws: WebSocketStream<S>,
    daemon: D,
    host_env: Arc<BTreeMap<String, String>>,
    shutdown: CancellationToken,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    D: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (mut ws_sink, mut ws_stream) = ws.split();
    let (daemon_read, mut daemon_write) = tokio::io::split(daemon);
    let (ws_tx, mut ws_rx) = mpsc::channel::<WsMessage>(OUTGOING_QUEUE);

    // Single WebSocket writer task: writes daemon messages, pongs, keepalive
    // pings, and the closing frame in order without interleaving.
    let ws_writer = async {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Skip the immediate first tick.
        ping.tick().await;
        loop {
            tokio::select! {
                message = ws_rx.recv() => match message {
                    Some(message) => {
                        let is_close = matches!(message, WsMessage::Close(_));
                        if ws_sink.send(message).await.is_err() {
                            break;
                        }
                        if is_close {
                            let _ = ws_sink.close().await;
                            break;
                        }
                    }
                    None => {
                        let _ = ws_sink.close().await;
                        break;
                    }
                },
                _ = ping.tick() => {
                    if ws_sink.send(WsMessage::Ping(Bytes::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
    };

    // Daemon -> WebSocket: read NDJSON lines and emit one Text frame per line.
    let daemon_to_ws = {
        let ws_tx = ws_tx.clone();
        async move {
            let mut reader = MessageReader::new(daemon_read);
            loop {
                match reader.read_line().await {
                    Ok(Some(line)) => match String::from_utf8(line) {
                        Ok(text) => {
                            if ws_tx.send(WsMessage::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::debug!(%error, "daemon sent non-UTF-8 line");
                            let _ = ws_tx
                                .send(WsMessage::Close(Some(CloseFrame {
                                    code: CloseCode::Error,
                                    reason: "daemon sent invalid UTF-8".into(),
                                })))
                                .await;
                            break;
                        }
                    },
                    Ok(None) => {
                        let _ = ws_tx
                            .send(WsMessage::Close(Some(CloseFrame {
                                code: CloseCode::Normal,
                                reason: "daemon closed connection".into(),
                            })))
                            .await;
                        break;
                    }
                    Err(FrameError::LineTooLong { .. }) => {
                        let _ = ws_tx
                            .send(WsMessage::Close(Some(CloseFrame {
                                code: CloseCode::Size,
                                reason: "daemon message exceeded 16 MiB limit".into(),
                            })))
                            .await;
                        break;
                    }
                    Err(error) => {
                        tracing::debug!(%error, "daemon read failed");
                        let _ = ws_tx
                            .send(WsMessage::Close(Some(CloseFrame {
                                code: CloseCode::Error,
                                reason: "daemon stream failed".into(),
                            })))
                            .await;
                        break;
                    }
                }
            }
        }
    };

    // WebSocket -> Daemon: read Text frames, compact / apply host environment
    // defaults, and write one NDJSON line per frame.
    let ws_to_daemon = async {
        let mut close_code: Option<(CloseCode, &'static str)> = None;
        loop {
            let next = tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    close_code = Some((CloseCode::Away, "server shutting down"));
                    break;
                }
                next = ws_stream.next() => next,
            };
            match next {
                Some(Ok(WsMessage::Text(text))) => {
                    match prepare_daemon_line(text.as_str(), &host_env) {
                        Ok(mut line) => {
                            line.push(b'\n');
                            if daemon_write.write_all(&line).await.is_err()
                                || daemon_write.flush().await.is_err()
                            {
                                break;
                            }
                        }
                        Err(reason) => {
                            let error = Response {
                                id: None,
                                outcome: Err(RpcError::new(ErrorCode::InvalidRequest, reason)),
                            };
                            if let Ok(json) = encode_response(&error) {
                                let _ = ws_tx.send(WsMessage::Text(json.into())).await;
                            }
                            close_code = Some((CloseCode::Size, "message exceeds 16 MiB limit"));
                            break;
                        }
                    }
                }
                Some(Ok(WsMessage::Binary(_))) => {
                    close_code = Some((
                        CloseCode::Unsupported,
                        "rho-protocol requires UTF-8 JSON text frames",
                    ));
                    break;
                }
                Some(Ok(WsMessage::Ping(payload))) => {
                    if ws_tx.send(WsMessage::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(WsMessage::Pong(_))) | Some(Ok(WsMessage::Frame(_))) => {}
                Some(Ok(WsMessage::Close(_))) | None => break,
                Some(Err(error)) => {
                    tracing::debug!(%error, "WebSocket read failed");
                    break;
                }
            }
        }
        // Shut down the daemon write half so the daemon answers any requests
        // already read and then closes cleanly.
        let _ = daemon_write.shutdown().await;
        if let Some((code, reason)) = close_code {
            let _ = ws_tx
                .send(WsMessage::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                })))
                .await;
        }
        drop(ws_tx);
    };

    tokio::join!(ws_writer, daemon_to_ws, ws_to_daemon);
}

/// Converts one incoming WebSocket text frame into a single compact NDJSON line
/// (without the trailing `\n`) ready for the local daemon socket.
///
/// - Valid JSON is compacted to one line, so pretty-printed messages from
///   non-Rust clients work without breaking NDJSON framing.
/// - For methods that accept `ClientEnvironment`, `host_env` is used as the
///   base environment with any entries supplied by the client overlaid on top.
/// - Invalid JSON has raw `\r`/`\n` replaced by spaces so one WebSocket frame
///   can never smuggle multiple lines; the daemon still answers with a normal
///   JSON-RPC `ParseError`.
pub(super) fn prepare_daemon_line(
    frame: &str,
    host_env: &BTreeMap<String, String>,
) -> Result<Vec<u8>, String> {
    if frame.len() > MAX_LINE_BYTES {
        return Err(format!(
            "JSON-RPC message of {} bytes exceeds the {MAX_LINE_BYTES}-byte limit",
            frame.len()
        ));
    }
    let bytes = match serde_json::from_str::<Value>(frame) {
        Ok(mut value) => {
            apply_host_environment(&mut value, host_env);
            serde_json::to_vec(&value).expect("a serde_json::Value always serializes")
        }
        Err(_) => frame
            .bytes()
            .map(|byte| {
                if matches!(byte, b'\r' | b'\n') {
                    b' '
                } else {
                    byte
                }
            })
            .collect(),
    };
    if bytes.len() > MAX_LINE_BYTES {
        return Err(format!(
            "JSON-RPC message of {} bytes exceeds the {MAX_LINE_BYTES}-byte limit",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// For environment-bearing protocol requests, overlays client-provided
/// `params.environment` entries onto `host_env` (or supplies `host_env` when
/// `environment` is omitted or empty). Non-object `environment` values are
/// left untouched so the daemon still rejects them with `InvalidParams`.
fn apply_host_environment(message: &mut Value, host_env: &BTreeMap<String, String>) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return;
    };
    if !ENVIRONMENT_METHODS.contains(&method) {
        return;
    }
    let Some(params) = object.get_mut("params").and_then(Value::as_object_mut) else {
        return;
    };
    match params.get("environment") {
        None => {
            let merged = host_env
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect();
            params.insert("environment".to_owned(), Value::Object(merged));
        }
        Some(Value::Object(_)) => {
            let Some(Value::Object(client_env)) = params.remove("environment") else {
                unreachable!()
            };
            let mut merged: Map<String, Value> = host_env
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect();
            for (key, value) in client_env {
                merged.insert(key, value);
            }
            params.insert("environment".to_owned(), Value::Object(merged));
        }
        // Leave malformed non-object `environment` alone so the daemon returns
        // its normal `InvalidParams` error.
        Some(_) => {}
    }
}

fn encode_response(response: &Response) -> serde_json::Result<String> {
    let mut object = Map::new();
    object.insert("jsonrpc".to_owned(), Value::String("2.0".to_owned()));
    object.insert("id".to_owned(), Value::Null);
    if let Err(error) = &response.outcome {
        object.insert("error".to_owned(), serde_json::to_value(error)?);
    }
    serde_json::to_string(&Value::Object(object))
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
