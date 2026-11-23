//! Adapting message-framed transports (such as WebSockets) to the
//! newline-delimited `AsyncRead`/`AsyncWrite` stream [`crate::Client`] expects.
//!
//! Each frame carried by [`FrameSender`] and [`FrameReceiver`] is one complete
//! JSON-RPC 2.0 message with no trailing newline, matching the WebSocket wire
//! contract (`1 text frame = 1 JSON-RPC object`). Works identically on native
//! runtimes and `wasm32-unknown-unknown`.

use std::{
    io,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc,
};

/// Creates a paired [`FrameStream`] (for [`crate::Client::handshake`] or
/// [`crate::Client::handshake_with_io`]) and its message-framed endpoints
/// ([`FrameSender`] for frames arriving from the peer, [`FrameReceiver`] for
/// frames to send to the peer).
///
/// Both directions are unbounded, like the transport buffers they stand in
/// for: a [`crate::Client`] reads every incoming frame as it arrives, and
/// writes only its own requests.
pub fn frame_channel() -> (FrameStream, FrameSender, FrameReceiver) {
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    (
        FrameStream {
            incoming: in_rx,
            read_buf: Vec::new(),
            read_pos: 0,
            outgoing: Some(out_tx),
            write_buf: Vec::new(),
            shut_down: false,
            reader: None,
        },
        FrameSender { sender: in_tx },
        FrameReceiver { receiver: out_rx },
    )
}

/// Pushes incoming JSON-RPC text frames from a message-framed transport (such
/// as a WebSocket) into a [`FrameStream`].
///
/// Dropping the sender signals end-of-stream to the [`crate::Client`].
#[derive(Clone, Debug)]
pub struct FrameSender {
    sender: mpsc::UnboundedSender<String>,
}

impl FrameSender {
    /// Queues one complete JSON-RPC message received from the peer.
    ///
    /// # Errors
    ///
    /// [`FrameStreamClosed`] once the [`FrameStream`] has shut down or been
    /// dropped; the transport can then close.
    pub fn send(&self, frame: impl Into<String>) -> Result<(), FrameStreamClosed> {
        self.sender
            .send(frame.into())
            .map_err(|_| FrameStreamClosed)
    }
}

/// The [`FrameStream`] a [`FrameSender`] feeds has shut down or been dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStreamClosed;

impl std::fmt::Display for FrameStreamClosed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the frame stream is closed")
    }
}

impl std::error::Error for FrameStreamClosed {}

/// Receives outgoing JSON-RPC text frames written by a [`crate::Client`] to
/// forward over a message-framed transport (such as a WebSocket).
#[derive(Debug)]
pub struct FrameReceiver {
    receiver: mpsc::UnboundedReceiver<String>,
}

impl FrameReceiver {
    /// Returns the next complete JSON-RPC message (without a trailing newline),
    /// or `None` once the [`FrameStream`] has shut down or been dropped.
    pub async fn recv(&mut self) -> Option<String> {
        self.receiver.recv().await
    }
}

/// An `AsyncRead + AsyncWrite` stream backed by message-framed channels.
///
/// Shutting down the write side closes the [`FrameReceiver`] and ends reads
/// immediately (as [`crate::CloseOnShutdown`] does), because message-framed
/// transports like WebSockets have no half-close.
#[derive(Debug)]
pub struct FrameStream {
    incoming: mpsc::UnboundedReceiver<String>,
    read_buf: Vec<u8>,
    read_pos: usize,
    outgoing: Option<mpsc::UnboundedSender<String>>,
    write_buf: Vec<u8>,
    shut_down: bool,
    reader: Option<Waker>,
}

impl FrameStream {
    fn copy_buffered(&mut self, buf: &mut ReadBuf<'_>) -> bool {
        if self.read_pos >= self.read_buf.len() {
            return false;
        }
        let remaining = &self.read_buf[self.read_pos..];
        let to_copy = remaining.len().min(buf.remaining());
        buf.put_slice(&remaining[..to_copy]);
        self.read_pos += to_copy;
        if self.read_pos >= self.read_buf.len() {
            self.read_buf.clear();
            self.read_pos = 0;
        }
        true
    }
}

impl AsyncRead for FrameStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.shut_down {
            return Poll::Ready(Ok(()));
        }
        if this.copy_buffered(buf) {
            return Poll::Ready(Ok(()));
        }
        match this.incoming.poll_recv(cx) {
            Poll::Ready(Some(frame)) => {
                this.read_buf = frame_to_line(frame);
                this.read_pos = 0;
                this.copy_buffered(buf);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => {
                this.reader = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl AsyncWrite for FrameStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let Some(outgoing) = &this.outgoing else {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "frame stream is shut down",
            )));
        };
        this.write_buf.extend_from_slice(buf);
        while let Some(newline) = this.write_buf.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = this.write_buf.drain(..=newline).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let frame = String::from_utf8(line)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if outgoing.send(frame).is_err() {
                this.outgoing = None;
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "frame receiver closed",
                )));
            }
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.outgoing = None;
        this.shut_down = true;
        if let Some(reader) = this.reader.take() {
            reader.wake();
        }
        Poll::Ready(Ok(()))
    }
}

/// Converts one peer text frame into a single `\n`-terminated line so one
/// message frame can never split into multiple NDJSON lines.
fn frame_to_line(frame: String) -> Vec<u8> {
    let trimmed = frame.trim_end_matches(['\r', '\n']);
    if !trimmed.contains(['\r', '\n']) {
        let mut bytes = Vec::with_capacity(trimmed.len() + 1);
        bytes.extend_from_slice(trimmed.as_bytes());
        bytes.push(b'\n');
        return bytes;
    }
    // Pretty-printed JSON in one WebSocket frame compacts to a single line;
    // malformed input with raw newlines is flattened so it remains one frame
    // and produces a single parse error rather than smuggled messages.
    let mut bytes = match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => serde_json::to_vec(&value).expect("a serde_json::Value always serializes"),
        Err(_) => trimmed
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
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
#[path = "frame_stream_tests.rs"]
mod tests;
