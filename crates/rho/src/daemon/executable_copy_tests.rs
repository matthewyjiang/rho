use std::{fs, path::Path};

use pretty_assertions::assert_eq;

use super::stage;

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// Covers: a daemon started from the same install reuses its copy, and one
// started after an update runs the updated file, with the earlier copy
// removed instead of piling up per version.
// Owner: daemon executable staging.
#[test]
fn an_updated_executable_gets_a_fresh_copy_and_the_old_one_is_removed() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("install").join("rho.exe");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    let dir = temp.path().join("copies");
    fs::write(&executable, "rho 1").unwrap();

    let first = stage(&executable, &dir).unwrap();
    assert_eq!(stage(&executable, &dir).unwrap(), first);
    assert_eq!(first.extension(), executable.extension());

    fs::write(&executable, "rho 1.1").unwrap();
    let second = stage(&executable, &dir).unwrap();

    assert_eq!(fs::read_to_string(&second).unwrap(), "rho 1.1");
    assert_eq!(
        files_in(&dir),
        vec![second.file_name().unwrap().to_string_lossy().into_owned()]
    );
}
