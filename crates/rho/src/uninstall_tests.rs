use super::planning::{installation_with_evidence, removal};
use super::*;
use crate::installation::InstallationEvidence;
use pretty_assertions::assert_eq;
use std::io::{Cursor, Read};

#[test]
fn only_default_script_location_authorizes_executable_removal() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().to_path_buf();
    for (relative, hint, expected) in [
        (".local/bin/rho", None, Installation::Script),
        ("build/rho", None, Installation::Unknown),
        ("build/rho", Some("script"), Installation::Unknown),
        (".local/bin/other", Some("script"), Installation::Unknown),
        (
            ".local/bin/rho",
            Some("unrecognized"),
            Installation::Unknown,
        ),
        (
            ".local/bin/rho",
            Some("pacman"),
            Installation::Managed(ManagedInstallation::Pacman),
        ),
    ] {
        let paths = Paths {
            home: home.clone(),
            executable: home.join(relative),
            install_method: hint.map(str::to_owned),
            custom_data: None,
        };
        let expected = if !cfg!(unix) && expected == Installation::Script {
            Installation::Unknown
        } else {
            expected
        };
        assert_eq!(
            installation_with_evidence(
                &paths,
                InstallationEvidence {
                    managed: None,
                    cargo_metadata: false,
                    pacman_owned: false
                }
            ),
            expected
        );
    }
    // A package-owned executable can occupy the script's default directory.
    for evidence in [
        ManagedInstallation::Cargo {
            root: Some(home.join(".local")),
        },
        ManagedInstallation::Pacman,
        ManagedInstallation::Scoop(ScoopInstallScope::Global),
    ] {
        for hint in [None, Some("script"), Some("unknown"), Some("cargo")] {
            let paths = Paths {
                executable: home.join(".local/bin/rho"),
                home: home.clone(),
                install_method: hint.map(str::to_owned),
                custom_data: None,
            };
            assert_eq!(
                installation_with_evidence(
                    &paths,
                    InstallationEvidence {
                        managed: Some(evidence.clone()),
                        cargo_metadata: false,
                        pacman_owned: false
                    }
                ),
                Installation::Managed(evidence.clone())
            );
        }
    }
    let paths = Paths {
        executable: home.join(".local/bin/rho"),
        home,
        install_method: Some("script".into()),
        custom_data: None,
    };
    assert_eq!(
        installation_with_evidence(
            &paths,
            InstallationEvidence {
                managed: None,
                cargo_metadata: true,
                pacman_owned: false
            }
        ),
        Installation::Unknown
    );
}

#[test]
fn refuses_root_relative_traversal_and_wrong_target_types() {
    let temp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(temp.path()).unwrap();
    for path in [PathBuf::from("relative/.rho"), home.join("../.rho")] {
        assert!(validate_path(&path).is_err(), "{path:?}");
    }
    let root = home.ancestors().last().unwrap();
    assert!(validate_path(root).is_err());
    let file = home.join(".rho");
    fs::write(&file, "not a directory").unwrap();
    assert!(removal(file, TargetKind::DataDirectory).is_err());
    assert!(removal(home, TargetKind::Executable).is_err());
}

#[cfg(unix)]
#[test]
fn refuses_linked_purge_root_before_any_deletion() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "private").unwrap();
    let home = root.join("home");
    fs::create_dir(&home).unwrap();
    symlink(&outside, home.join(".rho")).unwrap();
    fs::create_dir_all(home.join(".local/bin")).unwrap();
    let executable = home.join(".local/bin/rho");
    fs::write(&executable, "binary").unwrap();
    let paths = Paths {
        home,
        executable: executable.clone(),
        install_method: None,
        custom_data: None,
    };
    assert!(run_with_io(
        &paths,
        /*purge*/ true,
        /*dry_run*/ false,
        &mut Cursor::new("yes\n"),
        &mut Vec::new()
    )
    .is_err());
    assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "private");
    assert!(executable.exists());
}

// Covers: purging data must not indirectly remove an unapproved executable.
// Owner: uninstall filesystem policy.
#[test]
fn refuses_purge_containing_the_running_executable() {
    let temp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(temp.path()).unwrap();
    let data = home.join(".rho");
    fs::create_dir(&data).unwrap();
    let executable = data.join("rho");
    fs::write(&executable, "binary").unwrap();
    fs::write(data.join("keep"), "user data").unwrap();
    let paths = Paths {
        home,
        executable: fs::canonicalize(&executable).unwrap(),
        install_method: None,
        custom_data: None,
    };
    assert!(run_with_io(
        &paths,
        /*purge*/ true,
        /*dry_run*/ false,
        &mut Cursor::new("yes\n"),
        &mut Vec::new()
    )
    .is_err());
    assert_eq!(fs::read_to_string(data.join("keep")).unwrap(), "user data");
    assert_eq!(fs::read_to_string(executable).unwrap(), "binary");
}

struct ChangeBeforeAnswer<F: FnOnce()> {
    answer: Cursor<&'static [u8]>,
    change: Option<F>,
}

impl<F: FnOnce()> Read for ChangeBeforeAnswer<F> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.answer.read(buffer)
    }
}

impl<F: FnOnce()> BufRead for ChangeBeforeAnswer<F> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if let Some(change) = self.change.take() {
            change();
        }
        self.answer.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.answer.consume(amount);
    }
}

// Covers: a target replaced during the confirmation prompt is not authorized.
// Owner: uninstall filesystem transaction, not the confirmation parser.
#[test]
fn changed_target_aborts_before_removing_anything() {
    for changed_kind in [TargetKind::DataDirectory, TargetKind::Executable] {
        if changed_kind == TargetKind::Executable && !cfg!(unix) {
            continue;
        }
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let data = home.join(".rho");
        fs::create_dir(&data).unwrap();
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        let executable = home.join(".local/bin/rho");
        fs::write(&executable, "binary").unwrap();
        let backup = home.join("original");
        let mut input = ChangeBeforeAnswer {
            answer: Cursor::new(b"yes\n".as_slice()),
            change: Some(|| match changed_kind {
                TargetKind::DataDirectory => {
                    fs::rename(&data, &backup).unwrap();
                    fs::create_dir(&data).unwrap();
                    fs::write(data.join("keep"), "replacement").unwrap();
                }
                TargetKind::Executable => {
                    fs::rename(&executable, &backup).unwrap();
                    fs::write(&executable, "replacement").unwrap();
                }
            }),
        };
        let paths = Paths {
            home,
            executable: executable.clone(),
            install_method: None,
            custom_data: None,
        };
        assert!(run_with_io(
            &paths,
            /*purge*/ true,
            /*dry_run*/ false,
            &mut input,
            &mut Vec::new()
        )
        .is_err());
        match changed_kind {
            TargetKind::DataDirectory => assert_eq!(
                fs::read_to_string(data.join("keep")).unwrap(),
                "replacement"
            ),
            TargetKind::Executable => {
                assert_eq!(fs::read_to_string(&executable).unwrap(), "replacement")
            }
        }
        assert!(data.exists());
        assert!(backup.exists());
        assert!(executable.exists());
    }
}

// Covers: a failed data purge must leave the executable available for retry.
// Owner: uninstall execution; injected I/O failure works even as root.
#[cfg(unix)]
#[test]
fn failed_purge_keeps_binary_and_uses_preview_order() {
    let temp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(temp.path()).unwrap();
    let data = home.join(".rho");
    let executable = home.join(".local/bin/rho");
    fs::create_dir(&data).unwrap();
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, "binary").unwrap();
    let paths = Paths {
        home,
        executable: executable.clone(),
        install_method: None,
        custom_data: None,
    };
    let plan = plan(&paths, /*purge*/ true).unwrap();
    assert_eq!(
        plan.removals
            .iter()
            .map(|target| (target.path.clone(), target.kind))
            .collect::<Vec<_>>(),
        vec![
            (data.clone(), TargetKind::DataDirectory),
            (executable.clone(), TargetKind::Executable),
        ]
    );
    let mut attempted = Vec::new();
    let error = execute(plan, &mut Vec::new(), |path, kind| {
        attempted.push((path.to_path_buf(), kind));
        match kind {
            TargetKind::DataDirectory => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected purge failure",
            )),
            TargetKind::Executable => fs::remove_file(path),
        }
    })
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(attempted, vec![(data.clone(), TargetKind::DataDirectory)]);
    assert_eq!(fs::read_to_string(executable).unwrap(), "binary");
    assert!(data.exists());
}
