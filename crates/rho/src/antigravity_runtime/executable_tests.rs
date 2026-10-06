use pretty_assertions::assert_eq;

use super::*;

// Covers: the harness pin points next to the canonical server (so a PATH
// symlink still finds it), defers to a user-set override, and is omitted
// when the harness is absent.
// Owner: Antigravity spawn env; the server's own search is the fallback.
#[cfg(unix)]
#[test]
fn harness_pin_follows_the_canonical_server() {
    let install = tempfile::tempdir().unwrap();
    let server = install.path().join(ANTIGRAVITY_PROGRAM);
    std::fs::write(&server, "").unwrap();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join(ANTIGRAVITY_PROGRAM);
    std::os::unix::fs::symlink(&server, &link).unwrap();

    assert_eq!(harness_env(Some(&link), None), vec![]);

    std::fs::write(install.path().join(HARNESS_FILE), "").unwrap();
    let harness = std::fs::canonicalize(install.path())
        .unwrap()
        .join(HARNESS_FILE);
    assert_eq!(
        harness_env(Some(&link), None),
        vec![(OsString::from(HARNESS_PATH_ENV), harness.into_os_string())]
    );
    assert_eq!(
        harness_env(Some(&link), Some(OsString::from("/custom/harness"))),
        vec![]
    );
}

// Covers: a frozen workflow launch spawns the server through a
// `/proc/self/fd/N` handle; the pin must still land next to the verified file.
// Owner: Antigravity spawn env for frozen launches.
#[cfg(target_os = "linux")]
#[test]
fn harness_pin_resolves_a_descriptor_backed_server() {
    use std::os::fd::AsRawFd as _;

    let install = tempfile::tempdir().unwrap();
    let server = install.path().join(ANTIGRAVITY_PROGRAM);
    std::fs::write(&server, "").unwrap();
    std::fs::write(install.path().join(HARNESS_FILE), "").unwrap();
    let handle = std::fs::File::open(&server).unwrap();
    let descriptor = PathBuf::from(format!("/proc/self/fd/{}", handle.as_raw_fd()));
    let harness = std::fs::canonicalize(install.path())
        .unwrap()
        .join(HARNESS_FILE);

    assert_eq!(
        harness_env(Some(&descriptor), None),
        vec![(OsString::from(HARNESS_PATH_ENV), harness.into_os_string())]
    );
}
