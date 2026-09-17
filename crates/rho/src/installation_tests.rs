use super::*;
use pretty_assertions::assert_eq;

// Covers: shared detection must distinguish package ownership from unrelated
// Cargo receipts, preserve custom roots, and route pacman-owned binaries.
// Owner: installation evidence, with injected package-manager probes.
#[test]
fn detects_package_evidence() {
    for (path, cargo_owns, pacman_owns, expected) in [
        (
            "/opt/rho/bin/rho",
            true,
            false,
            Some(ManagedInstallation::Cargo {
                root: Some("/opt/rho".into()),
            }),
        ),
        ("/home/me/.local/bin/rho", false, false, None),
        (
            "/home/me/.local/bin/rho",
            false,
            true,
            Some(ManagedInstallation::Pacman),
        ),
        (
            "/home/me/.cargo/bin/rho",
            false,
            false,
            Some(ManagedInstallation::Cargo {
                root: Some("/home/me/.cargo".into()),
            }),
        ),
    ] {
        assert_eq!(
            detect_with(
                Path::new(path),
                &ScoopRoots::default(),
                |_| cargo_owns,
                |_| pacman_owns,
            ),
            expected
        );
    }
    for (output, expected) in [
        (
            "ripgrep v14.1.1:\n    rg\nrho-coding-agent v0.12.3:\n    rho\n",
            true,
        ),
        ("rho-helper v0.1.0:\n    rho-helper\n", false),
        ("", false),
    ] {
        assert_eq!(cargo_install_list_contains_crate(output), expected);
    }
    // Default Cargo installs still use Cargo's default root when updating.
    assert_eq!(
        cargo_update_root_for_exe(Path::new("/home/me/.cargo/bin/rho"), |_| true),
        None
    );
    assert_eq!(
        cargo_update_root_for_exe(Path::new("/opt/rho/bin/rho"), |_| true),
        Some("/opt/rho".into())
    );
}

// Covers: custom Scoop roots must retain package-manager updates and global
// scope, without classifying unrelated files or prefix collisions as installs.
// Owner: installation evidence used by update and uninstall.
#[test]
fn detects_scoop_scope() {
    for (path, user, global, expected) in [
        (
            r"C:\Users\me\scoop\apps\rho\current\rho.exe",
            None,
            None,
            Some(ScoopInstallScope::User),
        ),
        (
            r"C:\Users\me\scoop\apps\rho\0.26.0\rho.exe",
            None,
            None,
            Some(ScoopInstallScope::User),
        ),
        (
            r"C:\Users\me\scoop\shims\rho.exe",
            None,
            None,
            Some(ScoopInstallScope::User),
        ),
        (
            r"C:\ProgramData\scoop\apps\rho\current\rho.exe",
            None,
            None,
            Some(ScoopInstallScope::Global),
        ),
        (
            r"C:\ProgramData\scoop\shims\rho.exe",
            None,
            None,
            Some(ScoopInstallScope::Global),
        ),
        (
            r"D:\tools\apps\rho\current\rho.exe",
            None,
            Some(r"D:\tools"),
            Some(ScoopInstallScope::Global),
        ),
        (
            r"D:\tools\scoop\apps\rho\current\rho.exe",
            None,
            Some(r"D:\tools\scoop"),
            Some(ScoopInstallScope::Global),
        ),
        (
            r"D:\tools\scoop\apps\rho\current\rho.exe",
            None,
            Some(r"D:\tool"),
            Some(ScoopInstallScope::User),
        ),
        (
            r"C:\Users\me\AppData\Local\Programs\rho\bin\rho.exe",
            None,
            None,
            None,
        ),
        (
            r"C:\Users\me\scoop\apps\git\current\bin\git.exe",
            None,
            None,
            None,
        ),
        (
            r"D:\tools\apps\rho\current\rho.exe",
            Some(r"D:\tools"),
            None,
            Some(ScoopInstallScope::User),
        ),
        (
            r"\\?\D:\tools\apps\rho\current\rho.exe",
            Some(r"D:\tools"),
            None,
            Some(ScoopInstallScope::User),
        ),
        (
            r"\\?\UNC\server\tools\apps\rho\current\rho.exe",
            None,
            Some(r"\\server\tools"),
            Some(ScoopInstallScope::Global),
        ),
        (
            r"D:\tools\shims\rho.exe",
            None,
            Some("d:/TOOLS/"),
            Some(ScoopInstallScope::Global),
        ),
        (
            r"D:\tools\apps\rho\2.11.0\rho.exe",
            Some(r"D:\tools"),
            Some(r"D:\tools"),
            Some(ScoopInstallScope::Global),
        ),
        (
            r"D:\tools-other\apps\rho\current\rho.exe",
            Some(r"D:\tools"),
            Some(r"D:\tools"),
            None,
        ),
        (
            r"D:\tools\apps\git\current\git.exe",
            Some(r"D:\tools"),
            Some(r"D:\tools"),
            None,
        ),
        (
            r"D:\tools\apps\rho\current\unrelated.exe",
            Some(r"D:\tools"),
            Some(r"D:\tools"),
            None,
        ),
        (
            r"D:\scoop\apps\rho\current\bin\unrelated.exe",
            None,
            None,
            None,
        ),
    ] {
        assert_eq!(
            detect_with(
                Path::new(path),
                &ScoopRoots {
                    user: user.map(str::to_owned),
                    global: global.map(str::to_owned),
                },
                |_| false,
                |_| false,
            ),
            expected.map(ManagedInstallation::Scoop),
            "{path}",
        );
    }
}
