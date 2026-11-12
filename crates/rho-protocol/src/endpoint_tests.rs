use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::*;

// Covers: the socket lives under $XDG_RUNTIME_DIR, in a directory named
// after its home, only when that variable is an absolute path; the home's id
// is the one every earlier rho derived, so clients and daemons of different
// versions still find each other.
// Owner: daemon endpoint layout.
#[test]
fn the_socket_follows_an_absolute_xdg_runtime_dir() {
    let home = Path::new("/home/user/.rho");
    for (xdg, expected) in [
        (
            Some("/run/user/1000"),
            "/run/user/1000/rho/55c3b4937aac39db",
        ),
        (Some(""), "/home/user/.rho/run"),
        (Some("relative/run"), "/home/user/.rho/run"),
        (None, "/home/user/.rho/run"),
    ] {
        let endpoint = DaemonEndpoint::locate(home, xdg.map(Path::new)).unwrap();
        assert_eq!(
            endpoint,
            DaemonEndpoint {
                runtime_dir: PathBuf::from(expected),
                socket: Path::new(expected).join("daemon.sock"),
            },
            "XDG_RUNTIME_DIR={xdg:?}"
        );
    }
}

// Covers: two rho homes of one user share $XDG_RUNTIME_DIR but never a
// socket, so neither daemon can reach, or remove, the other's.
// Owner: daemon endpoint layout.
#[test]
fn homes_sharing_an_xdg_runtime_dir_get_their_own_sockets() {
    let xdg = Some(Path::new("/run/user/1000"));
    let first = DaemonEndpoint::locate(Path::new("/home/user/.rho"), xdg).unwrap();
    let second = DaemonEndpoint::locate(Path::new("/home/other/.rho"), xdg).unwrap();

    assert_ne!(first.socket, second.socket);
    assert_eq!(first.runtime_dir.parent(), second.runtime_dir.parent());
}

// Covers: a socket path bind(2) would truncate fails up front, naming the
// limit and the actual length.
// Owner: daemon endpoint layout.
#[test]
fn overlong_socket_path_is_rejected_with_its_length() {
    let home = PathBuf::from(format!("/{}", "h".repeat(MAX_SOCKET_PATH_BYTES)));
    let socket = home.join("run").join("daemon.sock");

    let error = DaemonEndpoint::locate(&home, None).unwrap_err();

    assert_eq!(
        error,
        SocketPathTooLong {
            len: socket.as_os_str().len(),
            path: socket,
            max: MAX_SOCKET_PATH_BYTES,
        }
    );
}
