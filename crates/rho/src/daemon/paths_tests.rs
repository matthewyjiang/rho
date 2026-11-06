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

// Covers: every spelling of one home resolves to its one canonical path, so
// they all name the same socket as they share its one lock; otherwise a
// client spelling RHO_HOME differently could not reach the running daemon and
// a daemon it spawned would be refused the lock. A home that does not exist
// yet is created private first, so the daemon that creates it binds where its
// later clients look.
// Owner: daemon path policy (home resolution).
#[test]
fn every_spelling_of_a_home_resolves_to_its_canonical_path() {
    let root = TempDir::new().unwrap();
    let real_root = fs::canonicalize(root.path()).unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(home.join("sub")).unwrap();
    let link = root.path().join("link");
    symlink(&home, &link).unwrap();

    for (spelling, expected) in [
        (home.clone(), real_root.join("home")),
        (home.join(""), real_root.join("home")),
        (
            PathBuf::from(format!("{}//", home.display())),
            real_root.join("home"),
        ),
        (home.join("."), real_root.join("home")),
        (home.join("sub/.."), real_root.join("home")),
        (link.clone(), real_root.join("home")),
        // Missing, under a symlinked parent: created, then resolved.
        (link.join("new"), real_root.join("home/new")),
    ] {
        assert_eq!(real_home(&spelling).unwrap(), expected, "{spelling:?}");
    }
    let mode = fs::metadata(home.join("new")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}
