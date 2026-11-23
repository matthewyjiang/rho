//! WebSocket ↔ local daemon NDJSON relay for `rho daemon serve --http`.
//!
//! Wire contract over WebSocket:
//! - Every WebSocket **text frame** carries one complete JSON-RPC 2.0 object
//!   (no trailing newline required; compact JSON emitted toward the client).
//! - Maximum message/frame size matches [`rho_protocol::MAX_LINE_BYTES`]
//!   (16 MiB); a larger message closes with `Size` (1009).
//! - Binary frames are refused with Close code `Unsupported` (1003).
//! - A single incoming text frame is compacted to a single line before reaching
//!   the local daemon socket so pretty-printed JSON works cleanly and malformed
//!   input containing raw newlines can never smuggle a second JSON-RPC line.
//! - For the protocol methods that accept a `ClientEnvironment`, the bridge
//!   overlays the client's variables on top of the server host's environment
//!   (captured when the server starts), so remote clients on another OS can
//!   send `{}` or omit `environment` and still get the host's `PATH`/`HOME`.
//!
//! Closing has one owner: the relay decides the close frame once, after the
//! daemon has answered what it already read when that matters, and the writer
//! sends it last.

use std::{future::Future, time::Duration};

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use rho_protocol::{
    methods::{
        AUTH_CHOOSE_STORE, AUTH_KEY, AUTH_LIST, AUTH_LOGIN, AUTH_LOGOUT, MODEL_LIST, MODEL_REFRESH,
        SESSION_ATTACH, SESSION_NEW, SESSION_PROMPT,
    },
    FrameError, MessageReader, MAX_LINE_BYTES,
};
use serde_json::{Map, Value};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    time::Instant,
};
use tokio_tungstenite::{
    tungstenite::{
        protocol::{
            frame::{coding::CloseCode, CloseFrame},
            WebSocketConfig,
        },
        Error as WsError, Message as WsMessage,
    },
    WebSocketStream,
};

/// Frames queued toward one WebSocket client before the bridge stops reading
/// the daemon, so a slow client slows its own daemon connection instead of
/// growing bridge memory.
//
// Backpressure, not buffering: the daemon's connection writer queues its own
// output, so this only smooths bursts. 16 frames bound one stalled client at
// 16 × `MAX_LINE_BYTES` in the worst case, where 128 allowed 2 GiB.
const OUTGOING_QUEUE: usize = 16;

/// Keepalive ping interval, so reverse proxies and tunnels do not drop idle
/// attached connections.
//
// Half the 60 s idle timeout common proxies default to (nginx
// `proxy_read_timeout`, AWS ALB `idle_timeout`).
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// How long the bridge waits to hear anything from the client, a frame or
/// the pong to a keepalive ping, before treating the connection as dead.
//
// Two ping intervals: a live client answers each ping within one.
const IDLE_TIMEOUT: Duration = PING_INTERVAL.saturating_mul(2);

/// Protocol methods whose `params` include `environment: ClientEnvironment`.
/// A test holds this to the protocol schema.
const ENVIRONMENT_METHODS: [&str; 10] = [
    SESSION_NEW,
    SESSION_PROMPT,
    SESSION_ATTACH,
    AUTH_LIST,
    AUTH_KEY,
    AUTH_LOGOUT,
    AUTH_LOGIN,
    AUTH_CHOOSE_STORE,
    MODEL_LIST,
    MODEL_REFRESH,
];

/// WebSocket configuration sized to match `rho-protocol`'s 16 MiB message cap.
pub(super) fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_LINE_BYTES))
        .max_frame_size(Some(MAX_LINE_BYTES))
}

/// Relays one authenticated WebSocket connection to `daemon` until either
/// side closes or `draining` resolves. Draining stops reading the client,
/// delivers every answer the daemon still sends for requests it already
/// read, then closes with `Away` so the client reconnects.
pub(super) async fn relay_websocket<S, D>(
    ws: WebSocketStream<S>,
    daemon: D,
    host_env: &Map<String, Value>,
    draining: impl Future<Output = ()>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    D: AsyncRead + AsyncWrite + Unpin,
{
    let (ws_sink, ws_stream) = ws.split();
    let (daemon_read, daemon_write) = tokio::io::split(daemon);
    let (frames, outgoing) = mpsc::channel::<WsMessage>(OUTGOING_QUEUE);
    let relay = async move {
        let outbound = daemon_to_client(daemon_read, frames.clone());
        tokio::pin!(outbound);
        let first = tokio::select! {
            ended = client_to_daemon(ws_stream, daemon_write, host_env, draining) => First::Client(ended),
            close = &mut outbound => First::Daemon(close),
        };
        let close = match first {
            First::Client(Inbound::ClientGone) => None,
            First::Client(Inbound::Refused(close)) => Some(close),
            First::Client(Inbound::Draining) => {
                (&mut outbound).await;
                Some(close_frame(CloseCode::Away, "server going away".to_owned()))
            }
            First::Client(Inbound::DaemonGone) => Some(outbound.await),
            First::Daemon(close) => Some(close),
        };
        if let Some(close) = close {
            let _ = frames.send(WsMessage::Close(Some(close))).await;
        }
    };
    tokio::join!(write_frames(ws_sink, outgoing), relay);
}

/// Which direction of the relay ended first.
enum First {
    Client(Inbound),
    Daemon(CloseFrame),
}

/// Why the client-to-daemon direction ended.
enum Inbound {
    /// The client closed or vanished; nothing more can reach it.
    ClientGone,
    /// The client broke the wire contract; close at once with this frame.
    Refused(CloseFrame),
    /// The server is draining; deliver the daemon's remaining answers first.
    Draining,
    /// The daemon stopped accepting requests; its side decides the close.
    DaemonGone,
}

/// Sends every queued frame, plus keepalive pings, until the queue closes or
/// the client stops accepting frames. Owning `outgoing` means returning
/// early closes the queue, which stops the daemon-to-client direction.
async fn write_frames<S>(
    mut sink: SplitSink<WebSocketStream<S>, WsMessage>,
    mut outgoing: mpsc::Receiver<WsMessage>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut ping = tokio::time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let message = tokio::select! {
            message = outgoing.recv() => match message {
                Some(message) => message,
                None => break,
            },
            _ = ping.tick() => WsMessage::Ping(Default::default()),
        };
        if sink.send(message).await.is_err() {
            return;
        }
    }
    let _ = sink.close().await;
}

/// Forwards client text frames to the daemon, one NDJSON line each, until
/// the client stops, breaks the wire contract, or `draining` resolves. Shuts
/// the daemon write half on the way out, so the daemon answers what it
/// already read and then closes its side.
async fn client_to_daemon<S, W>(
    mut stream: SplitStream<WebSocketStream<S>>,
    mut daemon: W,
    host_env: &Map<String, Value>,
    draining: impl Future<Output = ()>,
) -> Inbound
where
    S: AsyncRead + AsyncWrite + Unpin,
    W: AsyncWrite + Unpin,
{
    tokio::pin!(draining);
    let ended = loop {
        let next = tokio::select! {
            biased;
            () = &mut draining => break Inbound::Draining,
            next = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => next,
        };
        let message = match next {
            Err(_) => {
                tracing::debug!(
                    "WebSocket client silent for {IDLE_TIMEOUT:?} (IDLE_TIMEOUT); dropping it"
                );
                break Inbound::ClientGone;
            }
            Ok(None) => break Inbound::ClientGone,
            Ok(Some(Err(WsError::Capacity(error)))) => {
                tracing::debug!(%error, "WebSocket client sent an oversized message");
                break Inbound::Refused(too_large("message"));
            }
            Ok(Some(Err(error))) => {
                tracing::debug!(%error, "WebSocket read failed");
                break Inbound::ClientGone;
            }
            Ok(Some(Ok(message))) => message,
        };
        match message {
            WsMessage::Text(text) => match prepare_daemon_line(text.as_str(), host_env) {
                Ok(line) => {
                    if daemon.write_all(&line).await.is_err() || daemon.flush().await.is_err() {
                        break Inbound::DaemonGone;
                    }
                }
                Err(TooLarge { bytes }) => {
                    break Inbound::Refused(too_large(&format!("message of {bytes} bytes")));
                }
            },
            WsMessage::Binary(_) => {
                break Inbound::Refused(close_frame(
                    CloseCode::Unsupported,
                    "rho-protocol requires UTF-8 JSON text frames".to_owned(),
                ));
            }
            WsMessage::Close(_) => break Inbound::ClientGone,
            // tungstenite answers pings itself; any frame proves the client lives.
            WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_) => {}
        }
    };
    let _ = daemon.shutdown().await;
    ended
}

/// Forwards daemon NDJSON lines to the client, one text frame each, until
/// the daemon closes its side or the client stops accepting frames. Returns
/// the close frame that describes how the daemon side ended.
async fn daemon_to_client<R>(daemon: R, frames: mpsc::Sender<WsMessage>) -> CloseFrame
where
    R: AsyncRead + Unpin,
{
    let mut reader = MessageReader::new(daemon);
    loop {
        let line = match reader.read_line().await {
            Ok(Some(line)) => line,
            Ok(None) => {
                return close_frame(CloseCode::Normal, "daemon closed the connection".to_owned())
            }
            Err(FrameError::LineTooLong { .. }) => {
                return too_large("a daemon message");
            }
            Err(error) => {
                tracing::debug!(%error, "daemon read failed");
                return close_frame(CloseCode::Error, "daemon stream failed".to_owned());
            }
        };
        let text = match String::from_utf8(line) {
            Ok(text) => text,
            Err(error) => {
                tracing::debug!(%error, "daemon sent a non-UTF-8 line");
                return close_frame(CloseCode::Error, "daemon sent invalid UTF-8".to_owned());
            }
        };
        if frames.send(WsMessage::Text(text.into())).await.is_err() {
            // The writer is gone, so no close frame can reach the client.
            return close_frame(CloseCode::Normal, "client gone".to_owned());
        }
    }
}

fn close_frame(code: CloseCode, reason: String) -> CloseFrame {
    CloseFrame {
        code,
        reason: reason.into(),
    }
}

/// The close for a message over [`MAX_LINE_BYTES`], naming the limit.
fn too_large(what: &str) -> CloseFrame {
    close_frame(
        CloseCode::Size,
        format!("{what} exceeds MAX_LINE_BYTES ({MAX_LINE_BYTES} bytes)"),
    )
}

/// A daemon line over [`MAX_LINE_BYTES`] once the host environment was
/// overlaid.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct TooLarge {
    pub(super) bytes: usize,
}

/// Converts one incoming WebSocket text frame into a single compact NDJSON
/// line, `\n` included, ready for the local daemon socket.
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
    host_env: &Map<String, Value>,
) -> Result<Vec<u8>, TooLarge> {
    let mut bytes = match serde_json::from_str::<Value>(frame) {
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
        return Err(TooLarge { bytes: bytes.len() });
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// For environment-bearing protocol requests, overlays client-provided
/// `params.environment` entries onto `host_env` (or supplies `host_env` when
/// `environment` is omitted). A non-object `environment` is left alone so
/// the daemon still rejects it with `InvalidParams`.
fn apply_host_environment(message: &mut Value, host_env: &Map<String, Value>) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    let takes_environment = object
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| ENVIRONMENT_METHODS.contains(&method));
    if !takes_environment {
        return;
    }
    let Some(params) = object.get_mut("params").and_then(Value::as_object_mut) else {
        return;
    };
    let environment = match params.remove("environment") {
        None => Value::Object(host_env.clone()),
        Some(Value::Object(client)) => {
            let mut merged = host_env.clone();
            merged.extend(client);
            Value::Object(merged)
        }
        Some(malformed) => malformed,
    };
    params.insert("environment".to_owned(), environment);
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
