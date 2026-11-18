use std::ffi::OsString;

use pretty_assertions::assert_eq;

// Covers: a re-execution passes on the arguments the server started with
// and drops the socket an earlier one handed it, so a server upgraded
// again and again hands on exactly one.
// Owner: rho mcp serve upgrade in place.
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
    ];
    for (given, kept) in cases {
        assert_eq!(super::without_listener_fd(given.into_iter()), kept);
    }
}
