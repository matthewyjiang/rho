use std::{cell::Cell, io::ErrorKind, os::unix::net::UnixDatagram};

use pretty_assertions::assert_eq;

use super::acknowledge_notice_boundary;

// Covers: a slow or closed PTY observer cannot block notification processing.
// Owner: Unix observation transport; PTY scenarios cannot cheaply saturate the
// kernel queue while independently checking that the TUI send has returned.
#[test]
fn boundary_observation_deduplicates_and_survives_backpressure_and_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("observer.sock");
    let receiver = UnixDatagram::bind(&path).unwrap();
    receiver.set_nonblocking(true).unwrap();
    let last = Cell::new(0);
    let mut packet = vec![0; format!("boundary:{}", usize::MAX).len()];

    acknowledge_notice_boundary(1, &last, &path);
    acknowledge_notice_boundary(1, &last, &path);
    let length = receiver.recv(&mut packet).unwrap();
    assert_eq!(&packet[..length], b"boundary:1");
    assert_eq!(
        receiver.recv(&mut packet).unwrap_err().kind(),
        ErrorKind::WouldBlock
    );

    // Fill to the actual kernel limit rather than assuming a platform queue size.
    // Use fresh senders so a sender's buffer limit cannot masquerade as a full
    // receiver queue while the observer's separate sender still has room.
    loop {
        let filler = UnixDatagram::unbound().unwrap();
        filler.set_nonblocking(true).unwrap();
        match filler.send_to(b"fill", &path) {
            Ok(_) => {}
            // BSD and macOS can report a full AF_UNIX queue as ENOBUFS.
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(libc::ENOBUFS) =>
            {
                break;
            }
            Err(error) => panic!("fill observer queue: {error}"),
        }
    }
    acknowledge_notice_boundary(2, &last, &path);
    assert_eq!(last.get(), 1, "unsent states remain eligible for retry");
    while receiver.recv(&mut packet).is_ok() {}
    acknowledge_notice_boundary(2, &last, &path);
    let length = receiver.recv(&mut packet).unwrap();
    assert_eq!(&packet[..length], b"boundary:2");

    acknowledge_notice_boundary(0, &last, &path);
    acknowledge_notice_boundary(2, &last, &path);
    let length = receiver.recv(&mut packet).unwrap();
    assert_eq!(&packet[..length], b"boundary:2");

    drop(receiver);
    acknowledge_notice_boundary(3, &last, &path);
    assert_eq!(last.get(), 2);
    std::fs::remove_file(&path).unwrap();
    acknowledge_notice_boundary(3, &last, &path);
    assert_eq!(last.get(), 2);
}
