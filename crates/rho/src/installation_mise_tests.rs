use super::*;
use pretty_assertions::assert_eq;

// Covers: `rho update` must upgrade the tool spec mise installed rho under,
// such as a `github:` backend, rather than guess one from the directory name.
// Owner: mise tool lookup.
#[test]
fn finds_mise_tool_by_install_path() {
    let ls = br#"{
        "jq": [{"install_path": "/mise/installs/jq/1.8.2"}],
        "github:matthewyjiang/rho": [
            {"install_path": "/mise/installs/github-matthewyjiang-rho/2.30.0"},
            {"install_path": "/mise/installs/github-matthewyjiang-rho/2.31.0"}
        ]
    }"#;
    for (json, install_path, expected) in [
        (
            &ls[..],
            "/mise/installs/github-matthewyjiang-rho/2.31.0",
            Some("github:matthewyjiang/rho"),
        ),
        (&ls[..], "/mise/installs/rho/2.31.0", None),
        (b"not json", "/mise/installs/rho/2.31.0", None),
    ] {
        assert_eq!(
            mise_tool_from_ls_json(json, Path::new(install_path))
                .ok()
                .as_deref(),
            expected,
            "{install_path}",
        );
    }
}

// Covers: a symlinked default mise data directory resolves the running
// executable to a path without `mise/installs`; it must still be mise's.
// Owner: mise install detection (filesystem).
#[cfg(unix)]
#[test]
fn detects_install_through_symlinked_default_data_dir() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("tool-storage");
    let version_dir = storage.join("installs/rho/2.31.0");
    std::fs::create_dir_all(&version_dir).unwrap();
    std::fs::write(version_dir.join("rho"), "").unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(home.join(".local/share")).unwrap();
    std::os::unix::fs::symlink(&storage, home.join(".local/share/mise")).unwrap();

    let roots = mise_installs_roots(|name| (name == "HOME").then(|| home.clone().into()));
    let exe =
        std::fs::canonicalize(home.join(".local/share/mise/installs/rho/2.31.0/rho")).unwrap();

    assert_eq!(
        mise_install_path(&exe, &roots),
        Some(std::fs::canonicalize(&version_dir).unwrap())
    );
}

// Covers: Windows `current_exe` and `MISE_DATA_DIR` can disagree on case;
// the install must still be mise's rather than the install script's.
// Owner: mise install detection (Windows paths).
#[cfg(windows)]
#[test]
fn matches_windows_roots_ignoring_case() {
    let root = PathBuf::from(r"c:\users\me\misedata\installs");
    for (path, expected) in [
        (
            r"C:\Users\Me\MiseData\installs\rho\2.31.0\rho.exe",
            Some(root.join(r"rho\2.31.0")),
        ),
        (
            r"C:\Users\Me\AppData\Local\MISE\Installs\rho\2.31.0\rho.exe",
            Some(PathBuf::from(
                r"C:\Users\Me\AppData\Local\MISE\Installs\rho\2.31.0",
            )),
        ),
        (r"C:\Users\Me\Other\installs\rho\2.31.0\rho.exe", None),
    ] {
        assert_eq!(
            mise_install_path(Path::new(path), std::slice::from_ref(&root)),
            expected,
            "{path}"
        );
    }
}
