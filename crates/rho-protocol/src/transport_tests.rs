use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

// Covers: on a transport where shutting down the write side tells the peer
// nothing, a reader already parked waiting for data is woken and ends once
// the write side shuts down, so both halves drop and the peer sees the
// connection close. Without it a dropped client and the daemon wait for each
// other for good.
// Owner: protocol client transport.
#[tokio::test]
async fn a_parked_reader_ends_once_the_write_side_shuts_down() {
    // The peer end stays open, as a named pipe's does.
    let (stream, mut peer) = tokio::io::duplex(64);
    let (mut read, mut write) = tokio::io::split(CloseOnShutdown::new(stream));
    let (started, reading) = tokio::sync::oneshot::channel();
    // Its own task: once parked in the read, only a wake runs it again.
    let reader = tokio::spawn(async move {
        let first = read.read_u8().await;
        started.send(()).unwrap();
        let mut rest = Vec::new();
        let end = read.read_to_end(&mut rest).await;
        (first.ok(), end.ok(), rest)
    });
    peer.write_u8(b'h').await.unwrap();
    // On this test's single thread the reader runs on from its signal until
    // it parks in `read_to_end`, before this task continues.
    reading.await.unwrap();

    write.shutdown().await.unwrap();

    assert_eq!(reader.await.unwrap(), (Some(b'h'), Some(0), Vec::new()));
}
