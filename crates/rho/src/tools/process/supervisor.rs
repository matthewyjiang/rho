use super::{
    manager::{RetainedChunk, SharedRecord},
    platform::ProcessTree,
    types::{Chunk, ProcessLimits, State, Stream},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc,
};
async fn reader<R: AsyncRead + Unpin>(
    stream: Stream,
    mut r: R,
    tx: mpsc::Sender<(Stream, Vec<u8>)>,
) {
    let mut b = [0; 8192];
    while let Ok(n) = r.read(&mut b).await {
        if n == 0 {
            break;
        }
        if tx.send((stream, b[..n].to_vec())).await.is_err() {
            break;
        }
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "supervisor owns the child and its distinct I/O and control channels"
)]
pub(super) async fn supervise(
    rec: SharedRecord,
    mut child: tokio::process::Child,
    stdout: impl AsyncRead + Unpin + Send + 'static,
    stderr: impl AsyncRead + Unpin + Send + 'static,
    tx: mpsc::Sender<(Stream, Vec<u8>)>,
    mut rx: mpsc::Receiver<(Stream, Vec<u8>)>,
    mut stop: mpsc::UnboundedReceiver<Duration>,
    timeout: Option<Duration>,
    limits: ProcessLimits,
    tree: Arc<ProcessTree>,
    exited: Arc<tokio::sync::Notify>,
) {
    tokio::spawn(reader(Stream::Stdout, stdout, tx.clone()));
    tokio::spawn(reader(Stream::Stderr, stderr, tx));
    let mut final_state = State::Exited;
    // A `pending` deadline makes the timeout arm inert when no timeout is set,
    // so both modes share one select loop.
    let deadline = async {
        match timeout {
            Some(timeout) => tokio::time::sleep(timeout).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            Some((stream, bytes)) = rx.recv() => push(&rec, stream, bytes, &limits),
            grace = stop.recv() => {
                final_state = State::Terminated;
                tree.terminate(&mut child, grace.unwrap_or_default()).await;
                break;
            }
            _ = &mut deadline => {
                final_state = State::TimedOut;
                tree.terminate(&mut child, Duration::ZERO).await;
                break;
            }
            status = child.wait() => {
                record_exit(&rec, status);
                // The leader can exit while descendants survive (`sleep
                // 300 & exit 0`). End the whole group so the drain below
                // always reaches EOF and no descendant outlives the
                // record, matching the exact-process adapter.
                tree.kill();
                break;
            }
        }
    }
    loop {
        tokio::select! {
            output = rx.recv() => {
                let Some((stream, bytes)) = output else { break };
                push(&rec, stream, bytes, &limits);
            }
            grace = stop.recv() => {
                if let Some(grace) = grace {
                    final_state = State::Terminated;
                    tree.terminate(&mut child, grace).await;
                }
                break;
            }
        }
    }
    let mut r = rec.lock().unwrap();
    r.stop = None;
    r.tree = None;
    r.state = final_state;
    r.completed = Some(Instant::now());
    r.notify.notify_waiters();
    drop(r);
    exited.notify_waiters();
}
fn record_exit(rec: &SharedRecord, status: std::io::Result<std::process::ExitStatus>) {
    let mut r = rec.lock().unwrap();
    r.exit_code = status.ok().and_then(|status| status.code());
}
fn push(rec: &SharedRecord, stream: Stream, b: Vec<u8>, limits: &ProcessLimits) {
    let mut r = rec.lock().unwrap();
    let len = b.len();
    let cursor = r.next;
    r.next += 1;
    r.bytes += len;
    r.chunks.push_back(RetainedChunk {
        chunk: Chunk {
            cursor,
            stream,
            text: String::from_utf8_lossy(&b).into_owned(),
        },
        byte_cost: len,
    });
    while r.bytes > limits.max_bytes || r.chunks.len() > limits.max_chunks {
        if let Some(c) = r.chunks.pop_front() {
            r.bytes -= c.byte_cost
        }
    }
    r.notify.notify_waiters();
}
