//! Adapting a transport to what [`crate::Client`] expects of one.

use std::{
    io,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A stream that ends its reads once its write side is shut down, for a
/// transport that cannot close one direction alone, such as a Windows named
/// pipe.
///
/// A [`crate::Client`] that is dropped shuts down its write side and then
/// reads until the daemon, seeing the end of its input, closes the
/// connection. On a transport without a half-close the shutdown tells the
/// daemon nothing, so each side would wait for the other for good. With this
/// around the stream the client stops reading at once, both halves drop, and
/// the daemon sees the connection close. What the daemon sent after the
/// shutdown is not read.
#[derive(Debug)]
pub struct CloseOnShutdown<S> {
    stream: S,
    shut_down: bool,
    /// The reader waiting for data when the write side shuts down.
    reader: Option<Waker>,
}

impl<S> CloseOnShutdown<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            shut_down: false,
            reader: None,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CloseOnShutdown<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.shut_down {
            // Reading nothing is the end of the stream.
            return Poll::Ready(Ok(()));
        }
        let read = Pin::new(&mut this.stream).poll_read(cx, buf);
        if read.is_pending() {
            this.reader = Some(cx.waker().clone());
        }
        read
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CloseOnShutdown<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let shut_down = Pin::new(&mut this.stream).poll_shutdown(cx);
        if shut_down.is_ready() {
            this.shut_down = true;
            if let Some(reader) = this.reader.take() {
                reader.wake();
            }
        }
        shut_down
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
