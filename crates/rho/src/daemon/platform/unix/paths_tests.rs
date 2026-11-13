use std::{
    ffi::OsString,
    fs,
    os::unix::fs::{symlink, DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::DaemonPathError;
use crate::daemon::paths::{real_home, DaemonPaths};

fn resolve(rho_home: &Path, xdg_runtime_dir: Option<&str>) -> Result<DaemonPaths, DaemonPathError> {
    DaemonPaths::resolve(rho_home, |name| match name {
        "XDG_RUNTIME_DIR" => xdg_runtime_dir.map(OsString::from),
        other => panic!("unexpected env lookup {other}"),
    })
}

// Covers: the socket goes where `XDG_RUNTIME_DIR` from the environment puts
// it, while the lock and info file stay in the home whatever the runtime
// directory, so daemons started with different XDG_RUNTIME_DIRs for one home
// still exclude each other. Where in a runtime directory the socket goes is
// `rho_protocol::DaemonEndpoint`'s.
// Owner: daemon path policy.
#[test]
fn the_socket_follows_xdg_runtime_dir_and_the_lock_stays_home() {
    let home = Path::new("/home/user/.rho");
    for (xdg, runtime_root) in [
        (Some("/run/user/1000"), Path::new("/run/user/1000/rho")),
        (None, Path::new("/home/user/.rho")),
    ] {
        let paths = resolve(home, xdg).unwrap();
        assert_eq!(
            paths.runtime_dir.parent(),
            Some(runtime_root),
            "XDG_RUNTIME_DIR={xdg:?}"
        );
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

// Covers: a daemon that holds the lock binds over the socket a crashed one
// left behind, instead of failing on a path that is taken.
// Owner: daemon endpoint (Unix socket file).
#[tokio::test]
async fn bind_replaces_the_socket_a_crashed_daemon_left() {
    let root = TempDir::new().unwrap();
    let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
    paths.prepare_runtime_dir().unwrap();
    fs::write(&paths.socket, "left by a crashed daemon").unwrap();

    let _listener = super::Listener::bind(&paths).unwrap();

    assert_eq!(
        paths.inspect_socket(),
        Ok(crate::daemon::paths::SocketFound::Private)
    );
}

// Covers: every file the daemon and its host create through the boundary is
// readable and writable by its owner alone, whatever the umask would allow.
// Owner: daemon private files (Unix modes).
#[test]
fn files_are_created_for_their_owner_alone() {
    let root = TempDir::new().unwrap();
    type Open = fn(&Path) -> std::io::Result<fs::File>;
    let opens: [(&str, Open); 3] = [
        ("lock", super::open_lock_file),
        ("log", super::open_log_file),
        ("created", super::create_private_file),
    ];

    let modes = opens.map(|(name, open)| {
        let path = root.path().join(name);
        open(&path).unwrap();
        (
            name,
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
        )
    });

    assert_eq!(modes, [("lock", 0o600), ("log", 0o600), ("created", 0o600)]);
}
