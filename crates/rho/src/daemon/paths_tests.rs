use std::os::unix::fs::symlink;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

fn resolve(rho_home: &Path, xdg_runtime_dir: Option<&str>) -> Result<DaemonPaths, DaemonPathError> {
    resolve_with(rho_home, None, xdg_runtime_dir)
}

fn resolve_with(
    rho_home: &Path,
    rho_home_env: Option<&str>,
    xdg_runtime_dir: Option<&str>,
) -> Result<DaemonPaths, DaemonPathError> {
    DaemonPaths::resolve(rho_home, |name| match name {
        "RHO_HOME" => rho_home_env.map(OsString::from),
        "XDG_RUNTIME_DIR" => xdg_runtime_dir.map(OsString::from),
        other => panic!("unexpected env lookup {other}"),
    })
}

// Covers: the default home's socket lives under $XDG_RUNTIME_DIR only when it
// is a usable absolute path; an explicit RHO_HOME always keeps its own socket,
// so two homes never share one daemon. The lock stays in the home whatever
// the runtime directory, so daemons started with different XDG_RUNTIME_DIRs
// for one home still exclude each other.
// Owner: daemon path policy.
#[test]
fn runtime_dir_follows_the_rho_home_and_absolute_xdg_runtime_dir_but_the_lock_stays_home() {
    let home = Path::new("/home/user/.rho");
    for (rho_home_env, xdg, expected) in [
        (None, Some("/run/user/1000"), "/run/user/1000/rho"),
        (None, Some(""), "/home/user/.rho/run"),
        (None, Some("relative/run"), "/home/user/.rho/run"),
        (None, None, "/home/user/.rho/run"),
        (Some(""), Some("/run/user/1000"), "/run/user/1000/rho"),
        (
            Some("/home/user/.rho"),
            Some("/run/user/1000"),
            "/home/user/.rho/run",
        ),
    ] {
        let paths = resolve_with(home, rho_home_env, xdg).unwrap();
        assert_eq!(
            paths.runtime_dir,
            PathBuf::from(expected),
            "RHO_HOME={rho_home_env:?} XDG_RUNTIME_DIR={xdg:?}"
        );
        assert_eq!(paths.socket, paths.runtime_dir.join("daemon.sock"));
        assert_eq!(
            paths.lock,
            PathBuf::from("/home/user/.rho/daemon/daemon.lock"),
            "RHO_HOME={rho_home_env:?} XDG_RUNTIME_DIR={xdg:?}"
        );
    }
}

// Covers: a socket path bind(2) would truncate fails up front, naming the
// limit and the actual length.
// Owner: daemon path policy.
#[test]
fn overlong_socket_path_is_rejected_with_its_length() {
    let home = PathBuf::from(format!("/{}", "h".repeat(MAX_SOCKET_PATH_BYTES)));
    let socket = home.join("run").join("daemon.sock");

    let error = resolve(&home, None).unwrap_err();

    assert_eq!(
        error,
        DaemonPathError::SocketPathTooLong {
            len: socket.as_os_str().len(),
            path: socket,
            max: MAX_SOCKET_PATH_BYTES,
        }
    );
}

// Covers: the runtime directory is created private, and an existing one that
// others could reach (or that is not a real directory) is refused instead of
// used.
// Owner: daemon path policy (filesystem permissions).
#[test]
fn prepare_runtime_dir_creates_private_dir_and_refuses_exposed_ones() {
    enum Setup {
        Missing,
        Dir(u32),
        File,
        Symlink,
    }
    for (name, setup, expected_mode) in [
        ("missing", Setup::Missing, Ok(0o700)),
        ("private", Setup::Dir(0o700), Ok(0o700)),
        ("group readable", Setup::Dir(0o750), Err(Some(0o750))),
        ("world searchable", Setup::Dir(0o701), Err(Some(0o701))),
        ("regular file", Setup::File, Err(None)),
        ("symlink to a private dir", Setup::Symlink, Err(None)),
    ] {
        let root = TempDir::new().unwrap();
        let xdg = root.path().join("xdg");
        fs::create_dir(&xdg).unwrap();
        let paths = resolve(root.path(), xdg.to_str()).unwrap();
        let dir = &paths.runtime_dir;
        match setup {
            Setup::Missing => {}
            Setup::Dir(mode) => {
                fs::create_dir(dir).unwrap();
                fs::set_permissions(dir, fs::Permissions::from_mode(mode)).unwrap();
            }
            Setup::File => fs::write(dir, "").unwrap(),
            Setup::Symlink => {
                let target = root.path().join("target");
                fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
                symlink(&target, dir).unwrap();
            }
        }

        let result = paths.prepare_runtime_dir();

        match (result, expected_mode) {
            (Ok(()), Ok(mode)) => {
                let actual = fs::metadata(dir).unwrap().permissions().mode() & 0o777;
                assert_eq!(actual, mode, "{name}");
            }
            (Err(error), Err(Some(mode))) => assert_eq!(
                error,
                DaemonPathError::InsecureMode {
                    path: dir.clone(),
                    mode,
                },
                "{name}"
            ),
            (Err(error), Err(None)) => assert_eq!(
                error,
                DaemonPathError::NotADirectory { path: dir.clone() },
                "{name}"
            ),
            (result, expected) => panic!("{name}: got {result:?}, expected {expected:?}"),
        }
    }
}
