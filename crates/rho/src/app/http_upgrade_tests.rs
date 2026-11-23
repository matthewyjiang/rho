use std::{ffi::OsString, net::TcpListener};

use pretty_assertions::assert_eq;

// Covers: a re-execution passes on the arguments the server started with
// and drops the socket an earlier one handed it, so an HTTP server upgraded
// again and again hands on exactly one.
// Owner: shared HTTP in-place upgrade.
#[test]
fn a_relaunch_keeps_the_arguments_but_an_earlier_socket() {
    let args = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();
    let cases = [
        (
            args(&[
                "mcp",
                "serve",
                "--http",
                "127.0.0.1:8765",
                "--name",
                "lab-box",
            ]),
            args(&[
                "mcp",
                "serve",
                "--http",
                "127.0.0.1:8765",
                "--name",
                "lab-box",
            ]),
        ),
        (
            args(&[
                "mcp",
                "serve",
                "--http",
                "127.0.0.1:8765",
                "--listener-fd",
                "7",
            ]),
            args(&["mcp", "serve", "--http", "127.0.0.1:8765"]),
        ),
        (
            args(&[
                "daemon",
                "serve",
                "--http",
                "127.0.0.1:8766",
                "--allow-host",
                "rho.example.com",
                "--listener-fd",
                "9",
            ]),
            args(&[
                "daemon",
                "serve",
                "--http",
                "127.0.0.1:8766",
                "--allow-host",
                "rho.example.com",
            ]),
        ),
    ];
    for (given, kept) in cases {
        assert_eq!(super::without_listener_fd(given.into_iter()), kept);
    }
}

// Covers: adopting an inherited listener rejects a socket bound to a different
// address/port than `--http` requested, and accepts a matching address or port 0.
// Owner: shared HTTP in-place upgrade.
#[tokio::test]
async fn adopted_listener_checks_bound_address() {
    let std_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let local = std_listener.local_addr().unwrap();

    let wrong = std::net::SocketAddr::from(([127, 0, 0, 1], local.port().wrapping_add(1).max(1)));
    let dup = std_listener.try_clone().unwrap();
    assert!(super::adopted_listener(dup, wrong).is_err());

    let wildcard_port = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    let dup = std_listener.try_clone().unwrap();
    assert_eq!(
        super::adopted_listener(dup, wildcard_port)
            .unwrap()
            .local_addr()
            .unwrap(),
        local
    );

    assert_eq!(
        super::adopted_listener(std_listener, local)
            .unwrap()
            .local_addr()
            .unwrap(),
        local
    );
}
