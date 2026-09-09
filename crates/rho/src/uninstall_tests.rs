use super::*;
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
            Installation::Managed(String::new()),
        ),
    ] {
        let paths = Paths {
            home: home.clone(),
            executable: home.join(relative),
            install_method: hint.map(str::to_owned),
            custom_data: None,
        };
        let expected = if cfg!(windows) && expected == Installation::Script {
            Installation::Unknown
        } else {
            expected
        };
        // Only the deletion decision is contractual, not the manual instructions.
        assert_eq!(
            std::mem::discriminant(&installation(&paths)),
            std::mem::discriminant(&expected)
        );
    }
    // A cargo --root install can occupy the script's default directory.
    fs::create_dir_all(home.join(".local")).unwrap();
    fs::write(home.join(".local/.crates2.json"), "{}").unwrap();
    let paths = Paths {
        executable: home.join(".local/bin/rho"),
        home,
        install_method: Some("script".into()),
        custom_data: None,
    };
    assert!(matches!(installation(&paths), Installation::Managed(_)));
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
        change: Some(|| {
            fs::rename(&data, &backup).unwrap();
            fs::create_dir(&data).unwrap();
            fs::write(data.join("keep"), "replacement").unwrap();
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
    assert_eq!(
        fs::read_to_string(data.join("keep")).unwrap(),
        "replacement"
    );
    assert!(backup.exists());
    assert!(executable.exists());
}
