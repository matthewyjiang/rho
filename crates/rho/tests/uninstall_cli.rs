//! Process-level coverage of destructive confirmation and early CLI dispatch.
#![cfg(unix)]

use std::{
    fs,
    io::Write,
    os::unix::fs::symlink,
    process::{Command, Stdio},
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

// Covers: a script override must not permit deleting a package-owned binary,
// even at the default script location and without a local Cargo receipt.
// Owner: CLI process / package-manager probe integration.
#[test]
fn package_ownership_overrides_script_deletion_hint() {
    use std::os::unix::fs::PermissionsExt;
    for (owner, package) in [
        ("cargo", "rho-coding-agent"),
        ("pacman", "rho-coding-agent"),
        ("pacman", "other-package"),
    ] {
        let temp = TempDir::new().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let bin = home.join(".local/bin/rho");
        let tools = home.join("tools");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::create_dir(&tools).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_rho"), &bin).unwrap();
        for tool in ["cargo", "pacman"] {
            let script = if tool == owner {
                if tool == "cargo" {
                    format!("#!/bin/sh\nprintf '{package} v2.9.1:\\n    rho\\n'\n")
                } else {
                    format!("#!/bin/sh\nprintf '{package}\\n'\n")
                }
            } else {
                "#!/bin/sh\nexit 1\n".into()
            };
            let path = tools.join(tool);
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        // pacman probing is Linux-only, matching production detection.
        if owner == "pacman" && !cfg!(target_os = "linux") {
            continue;
        }
        let mut child = Command::new(&bin)
            .arg("uninstall")
            .env("HOME", &home)
            .env("PATH", &tools)
            .env("RHO_INSTALL_METHOD", "script")
            .env_remove("RHO_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Managed installs need no confirmation; ignore a closed input pipe.
        let _ = child.stdin.take().unwrap().write_all(b"yes\n");
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{owner}: {output:?}");
        assert!(
            bin.exists(),
            "{owner} ownership must prevent executable deletion"
        );
    }
}

// Covers: cancellation/preview must not delete files; explicit consent removes
// only the selected installation/data, even with broken configuration.
// Owner: CLI process and filesystem.
#[test]
fn uninstall_requires_consent_and_preserves_unselected_data() {
    for (input, flags, removed_binary, removed_data) in [
        ("", vec![], false, false),
        ("\n", vec!["--purge"], false, false),
        ("N\n", vec!["--purge"], false, false),
        ("maybe\nn\n", vec![], false, false),
        ("Y\n", vec![], true, false),
        ("maybe\nYeS\n", vec!["--purge"], true, true),
        ("", vec!["--purge", "--dry-run"], false, false),
    ] {
        let root = TempDir::new().unwrap();
        let home = root.path().canonicalize().unwrap().join("home");
        let bin = home.join(".local/bin/rho");
        let data = home.join(".rho");
        let shared = home.join(".agents/keep");
        let project = root.path().join("project");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(shared.parent().unwrap()).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(&shared, "shared definitions").unwrap();
        fs::write(project.join("keep"), "project data").unwrap();
        fs::write(data.join("config.toml"), "invalid toml [").unwrap();
        symlink(&project, data.join("linked-project")).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_rho"), &bin).unwrap();

        let mut child = Command::new(&bin)
            .arg("uninstall")
            .args(&flags)
            .env("HOME", &home)
            .env_remove("RHO_HOME")
            .env_remove("RHO_INSTALL_METHOD")
            .env_remove("RHO_INSTALL_DIR")
            .env_remove("CARGO_HOME")
            .current_dir(&project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{flags:?} {input:?}: {output:?}");
        assert_eq!(
            (bin.exists(), data.exists()),
            (!removed_binary, !removed_data),
            "{flags:?} {input:?}"
        );
        assert_eq!(fs::read_to_string(&shared).unwrap(), "shared definitions");
        assert_eq!(
            fs::read_to_string(project.join("keep")).unwrap(),
            "project data"
        );
    }
}
