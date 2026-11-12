use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

// Covers: on a transport where shutting down the write side tells the peer
// nothing, a reader already waiting for data ends once the write side shuts
// down, so both halves drop and the peer sees the connection close. Without
// it a dropped client and the daemon wait for each other for good.
// Owner: protocol client transport.
#[tokio::test]
async fn a_waiting_reader_ends_once_the_write_side_shuts_down() {
    // The peer end stays open and silent, as a named pipe's does.
    let (stream, mut peer) = tokio::io::duplex(64);
    let (mut read, mut write) = tokio::io::split(CloseOnShutdown::new(stream));
    let reader = tokio::spawn(async move {
        let mut received = Vec::new();
        read.read_to_end(&mut received).await.map(|_| received)
    });
    write.write_all(b"hello").await.unwrap();
    let mut sent = [0; 5];
    peer.read_exact(&mut sent).await.unwrap();

    write.shutdown().await.unwrap();

    assert_eq!(
        (reader.await.unwrap().unwrap(), &sent),
        (Vec::new(), b"hello")
    );
}
