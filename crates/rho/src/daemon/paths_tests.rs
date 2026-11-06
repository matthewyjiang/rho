use std::os::unix::fs::symlink;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

fn resolve(rho_home: &Path, xdg_runtime_dir: Option<&str>) -> Result<DaemonPaths, DaemonPathError> {
    DaemonPaths::resolve(rho_home, |name| match name {
        "XDG_RUNTIME_DIR" => xdg_runtime_dir.map(OsString::from),
        other => panic!("unexpected env lookup {other}"),
    })
}

// Covers: the socket lives under $XDG_RUNTIME_DIR, in a directory of its own
// home, only when that variable is a usable absolute path; the lock and info
// file stay in the home whatever the runtime directory, so daemons started
// with different XDG_RUNTIME_DIRs for one home still exclude each other.
// Owner: daemon path policy.
#[test]
fn the_socket_follows_an_absolute_xdg_runtime_dir_and_the_lock_stays_home() {
    let home = Path::new("/home/user/.rho");
    let in_xdg = PathBuf::from("/run/user/1000/rho").join(home_id(home));
    for (xdg, expected) in [
        (Some("/run/user/1000"), in_xdg.as_path()),
        (Some(""), Path::new("/home/user/.rho/run")),
        (Some("relative/run"), Path::new("/home/user/.rho/run")),
        (None, Path::new("/home/user/.rho/run")),
    ] {
        let paths = resolve(home, xdg).unwrap();
        assert_eq!(paths.runtime_dir, expected, "XDG_RUNTIME_DIR={xdg:?}");
        assert_eq!(paths.socket, paths.runtime_dir.join("daemon.sock"));
        assert_eq!(
            (paths.lock, paths.info_file),
            (
                PathBuf::from("/home/user/.rho/daemon/daemon.lock"),
                PathBuf::from("/home/user/.rho/daemon/daemon.json"),
            ),
            "XDG_RUNTIME_DIR={xdg:?}"
        );
    }
}

// Covers: two rho homes of one user share $XDG_RUNTIME_DIR but never a
// socket, so neither daemon can reach, or remove, the other's.
// Owner: daemon path policy.
#[test]
fn homes_sharing_an_xdg_runtime_dir_get_their_own_sockets() {
    let xdg = Some("/run/user/1000");
    let first = resolve(Path::new("/home/user/.rho"), xdg).unwrap();
    let second = resolve(Path::new("/home/other/.rho"), xdg).unwrap();

    assert_ne!(first.socket, second.socket);
    assert_eq!(first.runtime_dir.parent(), second.runtime_dir.parent());
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
        fs::create_dir(dir.parent().unwrap()).unwrap();
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

// Covers: every spelling of one home names the same socket, since they share
// its one lock; otherwise a client spelling RHO_HOME differently could not
// reach the running daemon and a daemon it spawned would be refused the lock.
// Owner: daemon path policy.
#[test]
fn every_spelling_of_a_home_names_the_same_socket() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    fs::create_dir(&home).unwrap();
    let link = root.path().join("link");
    symlink(&home, &link).unwrap();
    let missing = root.path().join("missing");
    let xdg = Some("/run/user/1000");
    let socket = |path: PathBuf| resolve(&path, xdg).unwrap().socket;

    for spelling in [
        home.join(""),
        PathBuf::from(format!("{}//", home.display())),
        home.join("."),
        link,
    ] {
        assert_eq!(
            socket(spelling.clone()),
            socket(home.clone()),
            "{spelling:?}"
        );
    }
    assert_eq!(socket(missing.join("")), socket(missing.clone()));

    // A home that does not exist yet, under a symlinked parent or reached
    // through `..`, keeps its socket once it is created.
    for spelling in [
        root.path().join("link/new"),
        root.path().join("home/sub/../new"),
    ] {
        let before = socket(spelling.clone());
        fs::create_dir_all(home.join("new")).unwrap();
        assert_eq!(before, socket(spelling.clone()), "{spelling:?}");
        assert_eq!(before, socket(home.join("new")), "{spelling:?}");
        fs::remove_dir(home.join("new")).unwrap();
    }
}
