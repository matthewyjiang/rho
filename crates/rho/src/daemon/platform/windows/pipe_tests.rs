use std::ffi::OsString;

use super::{probe, Opened, PipeServer};

// Covers: a server that stopped listening leaves no instance a client can
// still connect to. The instance it was accepting on stays open after the
// drop until the runtime sees its accept cancelled, which a current-thread
// runtime does not before this test's probe: a client of a restarting
// daemon connected to it and was then closed without an answer.
// Owner: daemon::platform::windows (pipe server).
#[tokio::test]
async fn a_dropped_server_leaves_nothing_to_connect_to() {
    let name = OsString::from(format!(
        r"\\.\pipe\rho-test-dropped-server-{}",
        std::process::id()
    ));
    let mut server = PipeServer::create(&name).unwrap();
    // Polled once, so the instance is accepting when the server drops.
    tokio::select! {
        biased;
        accepted = server.accept() => panic!("nobody connected: {:?}", accepted.map(drop)),
        () = std::future::ready(()) => {}
    }

    drop(server);

    assert!(
        !matches!(probe(&name), Ok(Opened::Client(_))),
        "a client connected to the dropped server"
    );
}
